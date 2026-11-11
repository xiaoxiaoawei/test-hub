package eda

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"sync"
	"time"
)

type cadenceExecRunner struct{}
type cadenceLog struct {
	mu        sync.Mutex
	file      *os.File
	remaining int
}

func (l *cadenceLog) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	n := len(p)
	if len(p) > l.remaining {
		p = p[:l.remaining]
	}
	l.remaining -= len(p)
	if len(p) > 0 {
		if _, e := l.file.Write(p); e != nil {
			return 0, e
		}
	}
	return n, nil
}

func (cadenceExecRunner) Run(ctx context.Context, spec cadenceProcessSpec) (int, error) {
	for _, path := range []string{spec.ReportPath, spec.ErrorPath, spec.PIDPath} {
		if path != "" {
			if e := os.Remove(path); e != nil && !os.IsNotExist(e) {
				return 0, e
			}
		}
	}
	logPath := filepath.Join(spec.Directory, spec.Phase+".log")
	f, e := os.OpenFile(logPath, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if e != nil {
		return 0, e
	}
	defer f.Close()
	log := &cadenceLog{file: f, remaining: 16 * 1024 * 1024}
	cmd := exec.CommandContext(ctx, spec.Executable, spec.Args...)
	cmd.Dir = spec.Directory
	cmd.Stdout = log
	cmd.Stderr = log
	// A launcher can leave a private descendant holding its inherited pipes.
	// Bound the copy goroutines too; cancellation must not wait indefinitely.
	cmd.WaitDelay = 2 * time.Second
	cadenceHideConsole(cmd)
	if e = cmd.Start(); e != nil {
		return 0, e
	}
	pid := cmd.Process.Pid
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	stop := func() (bool, error) {
		select {
		case waitErr := <-done:
			return false, waitErr
		default:
		}
		killErr := cmd.Process.Kill()
		waitErr := <-done
		if killErr != nil {
			if errors.Is(killErr, os.ErrProcessDone) {
				return false, waitErr
			}
			if waitErr != nil {
				return false, waitErr
			}
			return false, killErr
		}
		return true, waitErr
	}
	if spec.PIDPath != "" {
		if e = os.WriteFile(spec.PIDPath, []byte(strconv.Itoa(pid)), 0600); e != nil {
			_, _ = stop()
			return pid, e
		}
	}
	ticker := time.NewTicker(50 * time.Millisecond)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			_, _ = stop()
			return pid, fmt.Errorf("EDA超时或已取消（仅终止本次新进程，日志%s）：%w", logPath, ctx.Err())
		case err := <-done:
			if err != nil {
				return pid, fmt.Errorf("EDA进程失败，日志%s：%w", logPath, err)
			}
			if info, e := os.Stat(spec.ReportPath); e == nil && info.Size() > 0 {
				return pid, nil
			}
			return pid, errors.New("EDA退出但没有生成结构化回读报告；不认定生成成功")
		case <-ticker.C:
			if spec.ErrorPath != "" {
				if info, e := os.Stat(spec.ErrorPath); e == nil && info.Size() > 0 {
					_, _ = stop()
					return pid, fmt.Errorf("EDA脚本失败，请查看%s", spec.ErrorPath)
				}
			}
			if info, e := os.Stat(spec.ReportPath); e == nil && info.Size() > 0 {
				// Capture normally keeps its GUI process running after a Tcl script.
				// Only the child we started is stopped, never a PID from the report.
				// Allegro normally exits immediately after its report. Preserve a
				// natural nonzero exit instead of racing it with an intentional kill.
				select {
				case waitErr := <-done:
					if waitErr != nil {
						return pid, fmt.Errorf("EDA报告后异常退出，日志%s：%w", logPath, waitErr)
					}
					return pid, nil
				case <-ctx.Done():
					_, _ = stop()
					return pid, ctx.Err()
				case <-time.After(50 * time.Millisecond):
				}
				killed, waitErr := stop()
				if !killed && waitErr != nil {
					return pid, fmt.Errorf("EDA报告后异常退出，日志%s：%w", logPath, waitErr)
				}
				return pid, nil
			}
		}
	}
}
