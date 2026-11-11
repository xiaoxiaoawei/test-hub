package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"

	"packagestudio/internal/core"
)

var Version = "0.4.0"

type Settings struct {
	OutputDirectory  string `json:"outputDirectory"`
	Theme            string `json:"theme"`
	LibraryName      string `json:"libraryName"`
	IncludeSymbol    bool   `json:"includeSymbol"`
	IncludeFootprint bool   `json:"includeFootprint"`
	IncludeModel     bool   `json:"includeModel"`
	OpenAfterExport  bool   `json:"openAfterExport"`
	ExportTarget     string `json:"exportTarget"`
	PreviewContent   string `json:"previewContent"`
	PreviewLayout    string `json:"previewLayout"`
}
type Bootstrap struct {
	Settings     Settings             `json:"settings"`
	Parts        []core.Part          `json:"parts"`
	Version      string               `json:"version"`
	Capabilities map[string]bool      `json:"capabilities"`
	History      []ExportHistoryEntry `json:"history"`
}
type ExportRequest struct {
	PartIDs          []string `json:"partIds"`
	OutputDirectory  string   `json:"outputDirectory"`
	LibraryName      string   `json:"libraryName"`
	IncludeSymbol    bool     `json:"includeSymbol"`
	IncludeFootprint bool     `json:"includeFootprint"`
	Target           string   `json:"target"`
	IncludeModel     bool     `json:"includeModel"`
}
type Service struct {
	mu              sync.RWMutex
	searchMu        sync.Mutex
	searchSnapshots map[string]searchSnapshot
	exportMu        sync.Mutex
	dataDir         string
	settings        Settings
	parts           map[string]core.Part
	client          *http.Client
	history         []ExportHistoryEntry
	updateMu        sync.Mutex
	updateClient    *updateClientState
}

func New(dataDir string) (*Service, error) {
	if err := os.MkdirAll(dataDir, 0755); err != nil {
		return nil, err
	}
	s := &Service{dataDir: dataDir, parts: map[string]core.Part{}, client: &http.Client{Timeout: 35 * time.Second}}
	s.settings = Settings{Theme: "light", LibraryName: "MyParts", IncludeSymbol: true, IncludeFootprint: true, ExportTarget: "kicad", PreviewContent: "footprint", PreviewLayout: "split"}
	if b, err := os.ReadFile(filepath.Join(dataDir, "settings.json")); err == nil {
		_ = json.Unmarshal(b, &s.settings)
	}
	s.settings = normalizeSettings(s.settings)
	s.history = []ExportHistoryEntry{}
	if b, err := os.ReadFile(filepath.Join(dataDir, "history.json")); err == nil {
		var history []ExportHistoryEntry
		if json.Unmarshal(b, &history) == nil {
			if len(history) > 100 {
				history = history[:100]
			}
			s.history = history
		}
	}
	for _, p := range core.DemoParts() {
		s.parts[p.ID] = p
	}
	if b, err := os.ReadFile(filepath.Join(dataDir, "parts.json")); err == nil {
		var parts []core.Part
		if json.Unmarshal(b, &parts) == nil {
			for _, p := range parts {
				if p.ID != "" {
					s.parts[p.ID] = p
				}
			}
		}
	}
	return s, nil
}

func (s *Service) GetBootstrap() Bootstrap {
	s.mu.RLock()
	defer s.mu.RUnlock()
	parts := make([]core.Part, 0, len(s.parts))
	for _, p := range s.parts {
		parts = append(parts, p)
	}
	sort.Slice(parts, func(i, j int) bool { return parts[i].ID < parts[j].ID })
	return Bootstrap{Settings: s.settings, Parts: parts, Version: Version, Capabilities: map[string]bool{"kicad": true, "altium": altiumHelperPath() != "", "pads": true, "cadence": true}, History: cloneHistory(s.history)}
}

var lcscPattern = regexp.MustCompile(`(?i)^C[0-9]{2,12}$`)

func (s *Service) FetchPart(ctx context.Context, lcscID string) (core.Part, error) {
	id := strings.ToUpper(strings.TrimSpace(lcscID))
	if !lcscPattern.MatchString(id) {
		return core.Part{}, errors.New("请输入有效的立创 C 编号，例如 C25804")
	}
	url := "https://easyeda.com/api/products/" + id + "/components?version=6.4.19.5"
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return core.Part{}, err
	}
	req.Header.Set("User-Agent", "PackageStudio/"+Version)
	req.Header.Set("Accept", "application/json")
	resp, err := s.client.Do(req)
	if err != nil {
		return core.Part{}, fmt.Errorf("获取 %s 失败：%w", id, err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return core.Part{}, fmt.Errorf("器件数据服务返回 HTTP %d", resp.StatusCode)
	}
	raw, err := io.ReadAll(io.LimitReader(resp.Body, 32*1024*1024+1))
	if err != nil {
		return core.Part{}, err
	}
	if len(raw) > 32*1024*1024 {
		return core.Part{}, errors.New("器件数据超过 32 MB 限制")
	}
	p, err := core.ParseEasyEDA(raw, id)
	if err != nil {
		return core.Part{}, err
	}
	if err = s.put(p); err != nil {
		return core.Part{}, err
	}
	return p, nil
}
func (s *Service) ImportPartJSON(raw string) (core.Part, error) {
	if len(raw) > 32*1024*1024 {
		return core.Part{}, errors.New("导入 JSON 超过 32 MB 限制")
	}
	p, err := core.ParseEasyEDA([]byte(raw), "")
	if err != nil {
		return core.Part{}, err
	}
	if err = s.put(p); err != nil {
		return core.Part{}, err
	}
	return p, nil
}
func (s *Service) put(p core.Part) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if len(s.parts) >= 500 {
		if _, exists := s.parts[p.ID]; !exists {
			return errors.New("器件缓存最多保存 500 项")
		}
	}
	parts := make([]core.Part, 0, len(s.parts)+1)
	for id, v := range s.parts {
		if id != p.ID {
			parts = append(parts, v)
		}
	}
	parts = append(parts, p)
	if err := writeJSON(filepath.Join(s.dataDir, "parts.json"), parts); err != nil {
		return err
	}
	s.parts[p.ID] = p
	return nil
}

func (s *Service) RemovePart(partID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	parts := make([]core.Part, 0, len(s.parts))
	for id, p := range s.parts {
		if id != partID {
			parts = append(parts, p)
		}
	}
	if err := writeJSON(filepath.Join(s.dataDir, "parts.json"), parts); err != nil {
		return err
	}
	delete(s.parts, partID)
	return nil
}
func normalizeSettings(settings Settings) Settings {
	if settings.Theme != "dark" {
		settings.Theme = "light"
	}
	if strings.TrimSpace(settings.LibraryName) == "" {
		settings.LibraryName = "MyParts"
	}
	if settings.ExportTarget != "altium" && settings.ExportTarget != "pads" && settings.ExportTarget != "cadence" {
		settings.ExportTarget = "kicad"
	}
	if settings.PreviewContent != "symbol" {
		settings.PreviewContent = "footprint"
	}
	if settings.PreviewLayout != "2d" && settings.PreviewLayout != "3d" {
		settings.PreviewLayout = "split"
	}
	return settings
}
func (s *Service) SaveSettings(settings Settings) (Settings, error) {
	settings = normalizeSettings(settings)
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := writeJSON(filepath.Join(s.dataDir, "settings.json"), settings); err != nil {
		return settings, err
	}
	s.settings = settings
	return settings, nil
}
func (s *Service) ExportParts(req ExportRequest) (result core.ExportResult, exportErr error) {
	defer func() {
		if err := s.recordExport(req, result, exportErr); err != nil && exportErr == nil {
			result.Warnings = append(result.Warnings, "导出文件已生成，但历史记录保存失败："+err.Error())
		}
	}()
	if req.Target != "" && req.Target != "kicad" && req.Target != "altium" && req.Target != "pads" && req.Target != "cadence" {
		return core.ExportResult{}, errors.New("不支持的导出目标")
	}
	if strings.TrimSpace(req.OutputDirectory) == "" {
		return core.ExportResult{}, errors.New("请先选择导出目录")
	}
	if !filepath.IsAbs(req.OutputDirectory) {
		return core.ExportResult{}, errors.New("导出目录必须为绝对路径")
	}
	if len(req.PartIDs) == 0 {
		return core.ExportResult{}, errors.New("请选择至少一个器件")
	}
	if !req.IncludeSymbol && !req.IncludeFootprint {
		return core.ExportResult{}, errors.New("请至少选择原理图或 PCB 封装")
	}
	s.mu.RLock()
	parts := make([]core.Part, 0, len(req.PartIDs))
	seen := map[string]bool{}
	for _, id := range req.PartIDs {
		p, ok := s.parts[id]
		if !ok {
			s.mu.RUnlock()
			return core.ExportResult{}, fmt.Errorf("找不到器件 %s，请重新添加", id)
		}
		if !seen[id] {
			parts = append(parts, p)
			seen[id] = true
		}
	}
	s.mu.RUnlock()
	for _, p := range parts {
		if err := core.ValidatePart(p, req.IncludeSymbol, req.IncludeFootprint); err != nil {
			return core.ExportResult{}, fmt.Errorf("%s：%w", p.Name, err)
		}
	}
	s.exportMu.Lock()
	defer s.exportMu.Unlock()
	for i := range parts {
		if parts[i].Model == nil {
			continue
		}
		model := *parts[i].Model
		parts[i].Model = &model
		parts[i].Model.Path = ""
		if req.IncludeModel && req.IncludeFootprint && model.UUID != "" {
			ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
			path, err := s.downloadModel(ctx, model.UUID, "step")
			cancel()
			if err != nil {
				return core.ExportResult{}, fmt.Errorf("%s 的 STEP 模型下载失败：%w", parts[i].Name, err)
			}
			parts[i].Model.Path = path
		}
	}
	if req.Target == "altium" {
		return exportAltium(parts, req)
	}
	if req.Target == "pads" {
		return core.ExportPADS(parts, req.OutputDirectory, req.LibraryName, req.IncludeSymbol, req.IncludeFootprint)
	}
	if req.Target == "cadence" {
		return core.ExportCadence(parts, req.OutputDirectory, req.LibraryName, req.IncludeSymbol, req.IncludeFootprint)
	}
	return core.ExportKiCad(parts, req.OutputDirectory, req.LibraryName, req.IncludeSymbol, req.IncludeFootprint)
}
func writeJSON(path string, v any) error {
	b, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".package-studio-*.tmp")
	if err != nil {
		return err
	}
	name := tmp.Name()
	defer os.Remove(name)
	if _, err = tmp.Write(b); err != nil {
		tmp.Close()
		return err
	}
	if err = tmp.Close(); err != nil {
		return err
	}
	return os.Rename(name, path)
}
