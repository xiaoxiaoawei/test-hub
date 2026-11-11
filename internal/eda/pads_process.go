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
)

func padsHelperPath() string {
	exe, _ := os.Executable()
	cwd, _ := os.Getwd()
	for _, path := range []string{filepath.Join(filepath.Dir(exe), "adapters", "pads", "native.ps1"), filepath.Join(cwd, "helpers", "pads", "native.ps1")} {
		if info, err := os.Stat(path); err == nil && info.Mode().IsRegular() {
			return path
		}
	}
	return ""
}

type padsLimitedBuffer struct {
	bytes.Buffer
	overflow bool
}

func (b *padsLimitedBuffer) Write(data []byte) (int, error) {
	n := len(data)
	remaining := 1024*1024 - b.Len()
	if len(data) > remaining {
		b.overflow = true
		data = data[:max(0, remaining)]
	}
	_, _ = b.Buffer.Write(data)
	return n, nil
}

func padsRunJob(ctx context.Context, job padsJob) (padsEvidence, error) {
	var result padsEvidence
	if runtime.GOOS != "windows" {
		return result, errors.New("PADS SDK 调用只支持 Windows")
	}
	helper := padsHelperPath()
	if helper == "" {
		return result, errors.New("缺少 adapters/pads 原生适配器文件")
	}
	requestPath := filepath.Join(job.Root, "request-"+job.Phase+".json")
	responsePath := filepath.Join(job.Root, "response-"+job.Phase+".json")
	request := struct {
		padsJob
		Response string `json:"response"`
	}{job, responsePath}
	raw, err := json.MarshalIndent(request, "", "  ")
	if err != nil {
		return result, err
	}
	if err = os.WriteFile(requestPath, raw, 0600); err != nil {
		return result, err
	}
	windows := os.Getenv("SystemRoot")
	if !filepath.IsAbs(windows) {
		return result, errors.New("无法定位 Windows 32 位 PowerShell")
	}
	powershell := filepath.Join(windows, "SysWOW64", "WindowsPowerShell", "v1.0", "powershell.exe")
	if info, e := os.Stat(powershell); e != nil || !info.Mode().IsRegular() {
		return result, errors.New("缺少 32 位 Windows PowerShell；不会回落到任意 PATH 程序")
	}
	cmd := exec.CommandContext(ctx, powershell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "RemoteSigned", "-File", helper, "-Request", requestPath)
	padsHideConsole(cmd)
	var log padsLimitedBuffer
	cmd.Stdout = &log
	cmd.Stderr = &log
	err = cmd.Run()
	_ = os.WriteFile(filepath.Join(job.Root, "helper-"+job.Phase+".log"), log.Bytes(), 0600)
	if err != nil {
		if ctx.Err() != nil {
			return result, ctx.Err()
		}
		if strings.Contains(log.String(), "STUDIO_PADS_BUSY") {
			return result, errors.New("请先保存并关闭全部 PADS Logic/Layout，再重试原生导出；现有进程未被终止")
		}
		message := strings.TrimSpace(log.String())
		if len(message) > 2000 {
			message = message[len(message)-2000:]
		}
		return result, fmt.Errorf("PADS helper 失败：%w；%s", err, message)
	}
	if log.overflow {
		return result, errors.New("PADS helper 日志超出限制")
	}
	f, err := os.Open(responsePath)
	if err != nil {
		return result, fmt.Errorf("PADS helper 未提供验证报告：%w", err)
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, 16*1024*1024+1))
	if err != nil {
		return result, err
	}
	return parsePADSEvidence(data)
}

func parsePADSEvidence(data []byte) (padsEvidence, error) {
	var result padsEvidence
	if len(data) > 16*1024*1024 {
		return result, errors.New("PADS 验证报告超出 16 MiB")
	}
	decoder := json.NewDecoder(bytes.NewReader(bytes.TrimPrefix(data, []byte{0xef, 0xbb, 0xbf})))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&result); err != nil {
		return result, fmt.Errorf("PADS 验证报告无效：%w", err)
	}
	if err := decoder.Decode(new(any)); err != io.EOF {
		return result, errors.New("PADS 验证报告包含尾随数据")
	}
	return result, nil
}

// DetectPADS reports static installation discovery only. No SDK is loaded and
// no claim about its license, CLI behavior or export compatibility is made.
func DetectPADS() []Installation {
	result := []Installation{}
	if runtime.GOOS != "windows" {
		return result
	}
	roots := []string{}
	for _, key := range []string{"PADS_ROOT", "SDD_HOME"} {
		if value := os.Getenv(key); filepath.IsAbs(value) {
			roots = append(roots, value)
		}
	}
	roots = append(roots, padsRegistryRoots()...)
	for _, pattern := range []string{`C:\MentorGraphics\*\SDD_HOME`, `C:\MentorGraphics\PADSVX*`, `C:\Siemens\*\SDD_HOME`, `C:\PADS*\SDD_HOME`} {
		found, _ := filepath.Glob(pattern)
		roots = append(roots, found...)
	}
	seen := map[string]bool{}
	for _, root := range roots {
		for _, candidate := range []string{root, filepath.Join(root, "SDD_HOME")} {
			candidate = filepath.Clean(candidate)
			key := strings.ToLower(candidate)
			if seen[key] {
				continue
			}
			seen[key] = true
			layout := filepath.Join(candidate, "Programs", "powerpcb.exe")
			logic := filepath.Join(candidate, "Programs", "powerlogic.exe")
			if _, err := os.Stat(layout); err != nil {
				if _, e := os.Stat(logic); e != nil {
					continue
				}
			}
			sdk, err := inspectPADSSDK(candidate, true, true)
			installation := Installation{ID: candidate, Product: "PADS Classic", Root: candidate, Executable: layout, Architecture: "x86", Source: "本机路径/注册表", RuntimeVerified: false, Message: "静态发现，尚未调用 SDK 验证"}
			if err != nil {
				installation.Message = err.Error()
			} else {
				installation.Ready = true
				installation.Executable = sdk.Layout
			}
			if version, e := padsFileIdentity(layout); e == nil {
				installation.Version = version.Version
			}
			result = append(result, installation)
		}
	}
	return result
}

type padsIdentity struct{ Product, Description, Version string }
