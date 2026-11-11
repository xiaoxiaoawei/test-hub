package eda

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

func padsModelHelperPath() string {
	exe, _ := os.Executable()
	cwd, _ := os.Getwd()
	for _, p := range []string{filepath.Join(filepath.Dir(exe), "adapters", "pads", "models.ps1"), filepath.Join(cwd, "helpers", "pads", "models.ps1")} {
		if st, e := os.Stat(p); e == nil && st.Mode().IsRegular() {
			return p
		}
	}
	return ""
}
func padsRunModelJob(ctx context.Context, j padsModelJob) (padsModelEvidence, error) {
	var result padsModelEvidence
	if runtime.GOOS != "windows" {
		return result, errors.New("PADS Reuse SDK 只支持 Windows")
	}
	helper := padsModelHelperPath()
	if helper == "" {
		return result, errors.New("缺少 adapters/pads/models.ps1")
	}
	systemRoot := os.Getenv("SystemRoot")
	if !filepath.IsAbs(systemRoot) {
		return result, errors.New("无法定位 Windows PowerShell")
	}
	shell := filepath.Join(systemRoot, "SysWOW64", "WindowsPowerShell", "v1.0", "powershell.exe")
	request := filepath.Join(j.Root, "models-request-"+j.Phase+".json")
	response := filepath.Join(j.Root, "models-response-"+j.Phase+".json")
	if e := padsWriteJSON(request, struct {
		padsModelJob
		Response string `json:"response"`
	}{j, response}); e != nil {
		return result, e
	}
	cmd := exec.CommandContext(ctx, shell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "RemoteSigned", "-File", helper, "-Request", request)
	cmd.WaitDelay = 2 * time.Second
	padsHideConsole(cmd)
	var log padsLimitedBuffer
	cmd.Stdout = &log
	cmd.Stderr = &log
	e := cmd.Run()
	_ = os.WriteFile(filepath.Join(j.Root, "models-"+j.Phase+".log"), log.Bytes(), 0600)
	if e != nil {
		if ctx.Err() != nil {
			return result, ctx.Err()
		}
		s := strings.TrimSpace(log.String())
		if len(s) > 2000 {
			s = s[len(s)-2000:]
		}
		return result, fmt.Errorf("PADS Reuse helper 失败：%w；%s", e, s)
	}
	if log.overflow {
		return result, errors.New("PADS Reuse helper 日志超过上限")
	}
	f, e := os.Open(response)
	if e != nil {
		return result, e
	}
	defer f.Close()
	data, e := io.ReadAll(io.LimitReader(f, 16*1024*1024+1))
	if e != nil {
		return result, e
	}
	return parsePADSModelEvidence(data)
}
func parsePADSModelEvidence(data []byte) (padsModelEvidence, error) {
	var e padsModelEvidence
	if len(data) > 16*1024*1024 {
		return e, errors.New("PADS Reuse 报告过大")
	}
	decoder := json.NewDecoder(bytes.NewReader(bytes.TrimPrefix(data, []byte{0xef, 0xbb, 0xbf})))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&e); err != nil {
		return e, err
	}
	if err := decoder.Decode(new(any)); err != io.EOF {
		return e, errors.New("PADS Reuse 报告有尾随数据")
	}
	return e, nil
}
