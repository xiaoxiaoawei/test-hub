package service

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
	"strings"
	"time"

	"packagestudio/internal/core"
)

func altiumHelperPath() string {
	if _, err := exec.LookPath("dotnet"); err != nil {
		return ""
	}
	exe, _ := os.Executable()
	cwd, _ := os.Getwd()
	for _, path := range []string{filepath.Join(filepath.Dir(exe), "adapters", "altium", "PackageStudio.Altium.dll"), filepath.Join(cwd, "adapters", "altium", "PackageStudio.Altium.dll")} {
		if info, err := os.Stat(path); err == nil && !info.IsDir() {
			return path
		}
	}
	return ""
}
func exportAltium(parts []core.Part, req ExportRequest) (core.ExportResult, error) {
	helper := altiumHelperPath()
	if helper == "" {
		return core.ExportResult{}, errors.New("Altium 适配器不可用：请保留 adapters/altium 文件夹，并安装 .NET 10 桌面运行所需的 .NET Runtime")
	}
	if err := os.MkdirAll(req.OutputDirectory, 0755); err != nil {
		return core.ExportResult{}, err
	}
	stage, err := os.MkdirTemp(req.OutputDirectory, ".packagestudio-ad-")
	if err != nil {
		return core.ExportResult{}, err
	}
	defer os.RemoveAll(stage)
	destination := filepath.Join(req.OutputDirectory, core.SafeName(req.LibraryName)+"_AD_"+time.Now().Format("20060102_150405")+"_"+strings.TrimPrefix(filepath.Base(stage), ".packagestudio-ad-"))
	payload, err := json.Marshal(struct {
		Parts            []core.Part `json:"parts"`
		OutputDirectory  string      `json:"outputDirectory"`
		LibraryName      string      `json:"libraryName"`
		IncludeSymbol    bool        `json:"includeSymbol"`
		IncludeFootprint bool        `json:"includeFootprint"`
	}{parts, stage, req.LibraryName, req.IncludeSymbol, req.IncludeFootprint})
	if err != nil {
		return core.ExportResult{}, err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, "dotnet", helper)
	hideConsole(cmd)
	cmd.Stdin = bytes.NewReader(payload)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	stdout, err := cmd.Output()
	if err != nil {
		return core.ExportResult{}, fmt.Errorf("Altium 库生成失败：%s", strings.TrimSpace(stderr.String()))
	}
	var result core.ExportResult
	if err = json.Unmarshal(stdout, &result); err != nil {
		return result, fmt.Errorf("Altium 适配器返回格式异常：%w", err)
	}
	for _, p := range parts {
		for _, warning := range p.Warnings {
			result.Warnings = append(result.Warnings, p.Name+"："+warning)
		}
	}
	if req.IncludeFootprint && req.IncludeModel {
		for _, p := range parts {
			if p.Model == nil || p.Model.Path == "" {
				continue
			}
			dir := filepath.Join(stage, core.SafeName(req.LibraryName)+".3dshapes")
			if err = os.MkdirAll(dir, 0755); err != nil {
				return result, err
			}
			path := filepath.Join(dir, core.SafeName(p.Name+"_"+p.ID)+".step")
			src, err := os.Open(p.Model.Path)
			if err != nil {
				return result, err
			}
			dst, err := os.Create(path)
			if err != nil {
				src.Close()
				return result, err
			}
			_, copyErr := io.Copy(dst, src)
			closeErr := dst.Close()
			src.Close()
			if copyErr != nil {
				return result, copyErr
			}
			if closeErr != nil {
				return result, closeErr
			}
			result.Files = append(result.Files, path)
		}
		result.Warnings = append(result.Warnings, "STEP 模型已单独保存；当前 AD 适配器不会自动把模型嵌入 PcbLib，请在 AD 中关联并核对定位。")
	}
	seenWarnings := map[string]bool{}
	warnings := []string{}
	for _, warning := range result.Warnings {
		if !seenWarnings[warning] {
			seenWarnings[warning] = true
			warnings = append(warnings, warning)
		}
	}
	result.Warnings = warnings
	type auditPart struct {
		ID       string   `json:"id"`
		Name     string   `json:"name"`
		Source   string   `json:"source"`
		PinCount int      `json:"pinCount"`
		PadCount int      `json:"padCount"`
		Units    int      `json:"units"`
		Warnings []string `json:"warnings"`
	}
	audits := []auditPart{}
	for _, p := range parts {
		audits = append(audits, auditPart{p.ID, p.Name, p.Source, len(p.Symbol.Pins), len(p.Footprint.Pads), p.Symbol.Units, p.Warnings})
	}
	reportPath := filepath.Join(stage, "conversion-report.json")
	if err = writeJSON(reportPath, map[string]any{"target": "Altium", "generator": "Package Studio", "version": Version, "parts": audits, "warnings": warnings}); err != nil {
		return result, err
	}
	result.Files = append(result.Files, reportPath)
	readmePath := filepath.Join(stage, "使用说明.txt")
	readme := "封装工坊 · Altium 导出\n\n通过 Altium 打开本目录的 .SchLib / .PcbLib，在测试工程核对尺寸、引脚和封装关联。\n当前库已通过生成器自身的写入/回读检查，尚未在所有 AD 版本中放置验证。\n\nSTEP 模型（如有）独立保存在 .3dshapes 目录，需在 AD 中关联；当前没有嵌入模型。\n转换提示、数据来源和引脚/焊盘数量见 conversion-report.json。\n原理图电气类型及部分机械层角色需要在 AD 中复核。\n\n在线数据来源：嘉立创EDA / EasyEDA 官方库 https://lceda.cn/ / https://easyeda.com/\n\n" + strings.Join(warnings, "\n") + "\n"
	if err = os.WriteFile(readmePath, []byte(readme), 0644); err != nil {
		return result, err
	}
	result.Files = append(result.Files, readmePath)
	for i, path := range result.Files {
		rel, err := filepath.Rel(stage, path)
		if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(os.PathSeparator)) {
			return core.ExportResult{}, errors.New("Altium 适配器返回了批次目录外的文件")
		}
		result.Files[i] = filepath.Join(destination, rel)
	}
	if err = os.Rename(stage, destination); err != nil {
		return core.ExportResult{}, err
	}
	result.Directory = destination
	return result, nil
}
