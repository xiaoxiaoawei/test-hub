package main

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"

	wruntime "github.com/wailsapp/wails/v2/pkg/runtime"
	"packagestudio/internal/core"
	"packagestudio/internal/service"
	"packagestudio/internal/updater"
)

type App struct {
	ctx              context.Context
	service          *service.Service
	dataDir          string
	updateHealthPath string
	updateNonce      string
	healthOnce       sync.Once
}

func (a *App) startup(ctx context.Context) { a.ctx = ctx }
func (a *App) GetBootstrap() service.Bootstrap {
	boot := a.service.GetBootstrap()
	if a.updateHealthPath != "" {
		a.healthOnce.Do(func() { _ = updater.WriteHealth(a.dataDir, a.updateHealthPath, a.updateNonce, service.Version) })
	}
	return boot
}
func (a *App) CheckForUpdates() (updater.Info, error) {
	ctx := a.ctx
	if ctx == nil {
		ctx = context.Background()
	}
	return a.service.CheckForUpdates(ctx)
}
func (a *App) DownloadUpdate() (updater.Download, error) {
	ctx := a.ctx
	if ctx == nil {
		ctx = context.Background()
	}
	return a.service.DownloadUpdate(ctx)
}
func (a *App) InstallUpdate(path string) error {
	if a.ctx == nil {
		return errors.New("请在正式桌面程序中安装更新")
	}
	if e := a.service.StartUpdate(path); e != nil {
		return e
	}
	go func() { time.Sleep(300 * time.Millisecond); wruntime.Quit(a.ctx) }()
	return nil
}
func (a *App) GetExportHistory() []service.ExportHistoryEntry { return a.service.GetExportHistory() }
func (a *App) ClearExportHistory() error                      { return a.service.ClearExportHistory() }
func (a *App) RevealGeneratedFile(path string) error {
	if err := a.service.ValidateGeneratedFile(path); err != nil {
		return err
	}
	cmd := exec.Command("explorer.exe", "/select,"+filepath.Clean(path))
	if err := cmd.Start(); err != nil {
		return err
	}
	return cmd.Process.Release()
}
func (a *App) FetchPart(lcscID string) (core.Part, error) {
	ctx := a.ctx
	if ctx == nil {
		ctx = context.Background()
	}
	return a.service.FetchPart(ctx, lcscID)
}
func (a *App) ImportPartJSON(raw string) (core.Part, error) { return a.service.ImportPartJSON(raw) }
func (a *App) RemovePart(partID string) error               { return a.service.RemovePart(partID) }
func (a *App) ExportParts(req service.ExportRequest) (core.ExportResult, error) {
	return a.service.ExportParts(req)
}
func (a *App) SaveSettings(s service.Settings) (service.Settings, error) {
	return a.service.SaveSettings(s)
}
func (a *App) ChooseOutputDirectory() (string, error) {
	current := a.service.GetBootstrap().Settings.OutputDirectory
	if a.ctx == nil {
		return current, nil
	}
	return wruntime.OpenDirectoryDialog(a.ctx, wruntime.OpenDialogOptions{Title: "选择器件库导出目录", DefaultDirectory: current, CanCreateDirectories: true})
}
func (a *App) OpenOutputDirectory(path string) error {
	if !filepath.IsAbs(path) {
		return errors.New("请选择有效的绝对路径")
	}
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	if !info.IsDir() {
		return errors.New("只能打开文件夹")
	}
	cmd := exec.Command("explorer.exe", path)
	if err = cmd.Start(); err != nil {
		return err
	}
	return cmd.Process.Release()
}
func (a *App) LoadModel(partID string) (service.ModelResult, error) {
	ctx := a.ctx
	if ctx == nil {
		ctx = context.Background()
	}
	return a.service.LoadModel(ctx, partID)
}
