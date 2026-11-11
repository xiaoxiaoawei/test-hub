package main

import (
	"encoding/json"
	"fmt"
	"io/fs"
	"net"
	"net/http"
	"strings"
	"time"

	"packagestudio/internal/service"
)

func serveHTTP(addr string, app *App) error {
	host, _, err := net.SplitHostPort(addr)
	if err != nil {
		return err
	}
	if host != "127.0.0.1" && host != "localhost" && host != "::1" {
		return fmt.Errorf("开发服务只允许绑定回环地址")
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/api/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json; charset=utf-8")
		w.Header().Set("Cache-Control", "no-store")
		origin := r.Header.Get("Origin")
		if origin != "" && origin != "http://"+r.Host && origin != "http://localhost:5173" && origin != "http://127.0.0.1:5173" {
			http.Error(w, "forbidden origin", 403)
			return
		}
		if r.Host != addr && r.Host != "localhost:"+strings.Split(addr, ":")[len(strings.Split(addr, ":"))-1] {
			http.Error(w, "invalid host", 403)
			return
		}
		if r.Method != http.MethodGet && !strings.HasPrefix(r.Header.Get("Content-Type"), "application/json") {
			http.Error(w, "JSON content type required", 415)
			return
		}
		r.Body = http.MaxBytesReader(w, r.Body, 32*1024*1024)
		decode := func(v any) error { return json.NewDecoder(r.Body).Decode(v) }
		var result any
		var callErr error
		switch r.Method + " " + r.URL.Path {
		case "GET /api/bootstrap":
			result = app.GetBootstrap()
		case "GET /api/history":
			result = app.GetExportHistory()
		case "GET /api/update/check":
			result, callErr = app.CheckForUpdates()
		case "POST /api/update/download":
			result, callErr = app.DownloadUpdate()
		case "POST /api/update/install":
			var req struct {
				Path string `json:"path"`
			}
			callErr = decode(&req)
			if callErr == nil {
				callErr = app.InstallUpdate(req.Path)
				result = true
			}
		case "POST /api/history/clear":
			callErr = app.ClearExportHistory()
			result = true
		case "POST /api/reveal-file":
			var req struct {
				Path string `json:"path"`
			}
			callErr = decode(&req)
			if callErr == nil {
				callErr = app.RevealGeneratedFile(req.Path)
				result = true
			}
		case "POST /api/parts/fetch":
			var req struct {
				LCSCID string `json:"lcscID"`
			}
			callErr = decode(&req)
			if callErr == nil {
				result, callErr = app.service.FetchPart(r.Context(), req.LCSCID)
			}
		case "POST /api/parts/import":
			var req struct {
				Raw string `json:"raw"`
			}
			callErr = decode(&req)
			if callErr == nil {
				result, callErr = app.ImportPartJSON(req.Raw)
			}
		case "POST /api/export":
			var req service.ExportRequest
			callErr = decode(&req)
			if callErr == nil {
				result, callErr = app.ExportParts(req)
			}
		case "PUT /api/settings":
			var req service.Settings
			callErr = decode(&req)
			if callErr == nil {
				result, callErr = app.SaveSettings(req)
			}
		case "POST /api/dialog/directory":
			result, callErr = app.ChooseOutputDirectory()
		case "POST /api/open-directory":
			var req struct {
				Path string `json:"path"`
			}
			callErr = decode(&req)
			if callErr == nil {
				callErr = app.OpenOutputDirectory(req.Path)
				result = true
			}
		case "POST /api/model":
			var req struct {
				PartID string `json:"partId"`
			}
			callErr = decode(&req)
			if callErr == nil {
				result, callErr = app.service.LoadModel(r.Context(), req.PartID)
			}
		case "POST /api/parts/remove":
			var req struct {
				PartID string `json:"partId"`
			}
			callErr = decode(&req)
			if callErr == nil {
				callErr = app.RemovePart(req.PartID)
				result = true
			}
		default:
			http.NotFound(w, r)
			return
		}
		if callErr != nil {
			w.WriteHeader(http.StatusBadRequest)
			_ = json.NewEncoder(w).Encode(map[string]string{"error": callErr.Error()})
			return
		}
		_ = json.NewEncoder(w).Encode(result)
	})
	web, err := fs.Sub(assets, "uiassets")
	if err != nil {
		return err
	}
	mux.Handle("/", http.FileServer(http.FS(web)))
	server := &http.Server{Addr: addr, Handler: mux, ReadHeaderTimeout: 5 * time.Second, IdleTimeout: 60 * time.Second}
	return server.ListenAndServe()
}
