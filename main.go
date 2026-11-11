package main

import (
	"embed"
	"flag"
	"log"
	"os"
	"path/filepath"

	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	"github.com/wailsapp/wails/v2/pkg/options/windows"
	"packagestudio/internal/service"
	"packagestudio/internal/updater"
)

//go:embed all:uiassets
var assets embed.FS

func main() {
	serve := flag.String("serve", "", "仅本地测试：启动 HTTP 服务，例如 127.0.0.1:34116")
	dataDir := flag.String("data-dir", "", "指定应用数据目录")
	applyUpdate := flag.String("apply-update", "", "内部更新安装计划")
	updateHealth := flag.String("update-health", "", "内部更新启动确认")
	updateNonce := flag.String("update-nonce", "", "内部更新确认标记")
	flag.Parse()
	if *applyUpdate != "" {
		if err := updater.Apply(*applyUpdate); err != nil {
			log.Fatal(err)
		}
		return
	}
	if *dataDir == "" {
		cfg, err := os.UserConfigDir()
		if err != nil {
			log.Fatal(err)
		}
		*dataDir = filepath.Join(cfg, "PackageStudio")
	}
	s, err := service.New(*dataDir)
	if err != nil {
		log.Fatal(err)
	}
	app := &App{service: s, dataDir: *dataDir, updateHealthPath: *updateHealth, updateNonce: *updateNonce}
	if *serve != "" {
		if err = serveHTTP(*serve, app); err != nil {
			log.Fatal(err)
		}
		return
	}
	err = wails.Run(&options.App{
		Title: "封装工坊 · Package Studio", Width: 1440, Height: 940, MinWidth: 1080, MinHeight: 700,
		BackgroundColour: &options.RGBA{R: 245, G: 247, B: 249, A: 255},
		AssetServer:      &assetserver.Options{Assets: assets}, OnStartup: app.startup, Bind: []interface{}{app},
		Windows: &windows.Options{WebviewIsTransparent: false, WindowIsTranslucent: false, DisableWindowIcon: true, WebviewUserDataPath: filepath.Join(*dataDir, "webview2")},
	})
	if err != nil {
		log.Fatal(err)
	}
}
