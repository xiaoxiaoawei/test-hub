package service

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

type ModelResult struct {
	PartID   string   `json:"partId"`
	UUID     string   `json:"uuid"`
	OBJ      string   `json:"obj"`
	Warnings []string `json:"warnings"`
}

var uuidPattern = regexp.MustCompile(`^[a-fA-F0-9]{32}$`)

func (s *Service) LoadModel(ctx context.Context, partID string) (ModelResult, error) {
	s.mu.RLock()
	p, ok := s.parts[partID]
	s.mu.RUnlock()
	if !ok {
		return ModelResult{}, errors.New("器件不存在")
	}
	if p.Model == nil || !uuidPattern.MatchString(p.Model.UUID) {
		return ModelResult{}, errors.New("该器件没有可获取的三维模型")
	}
	path, err := s.downloadModel(ctx, p.Model.UUID, "obj")
	if err != nil {
		return ModelResult{}, err
	}
	b, err := os.ReadFile(path)
	if err != nil {
		return ModelResult{}, err
	}
	return ModelResult{PartID: partID, UUID: p.Model.UUID, OBJ: string(b), Warnings: []string{"三维模型来自器件数据源，装配方向和尺寸请与器件手册核对。"}}, nil
}
func (s *Service) downloadModel(ctx context.Context, uuid, format string) (string, error) {
	if !uuidPattern.MatchString(uuid) {
		return "", errors.New("模型标识无效")
	}
	dir := filepath.Join(s.dataDir, "models")
	if err := os.MkdirAll(dir, 0755); err != nil {
		return "", err
	}
	path := filepath.Join(dir, strings.ToLower(uuid)+"."+format)
	if info, err := os.Stat(path); err == nil && info.Size() > 16 {
		return path, nil
	}
	url := "https://modules.easyeda.com/3dmodel/" + uuid
	if format == "step" {
		url = "https://modules.easyeda.com/qAxj6KHrDKw4blvCG8QJPs7Y/" + uuid
	} else if format != "obj" {
		return "", errors.New("不支持的模型格式")
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return "", err
	}
	req.Header.Set("User-Agent", "PackageStudio/"+Version)
	resp, err := s.client.Do(req)
	if err != nil {
		return "", fmt.Errorf("三维模型下载失败：%w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("模型服务返回 HTTP %d", resp.StatusCode)
	}
	b, err := io.ReadAll(io.LimitReader(resp.Body, 64*1024*1024+1))
	if err != nil {
		return "", err
	}
	if len(b) > 64*1024*1024 {
		return "", errors.New("模型超过 64 MB 限制")
	}
	if format == "step" && !strings.Contains(string(b[:min(len(b), 256)]), "ISO-10303-21") {
		return "", errors.New("模型服务没有返回有效 STEP 文件")
	}
	if format == "obj" && !strings.Contains(string(b), "\nv ") {
		return "", errors.New("模型服务没有返回有效 OBJ 文件")
	}
	tmp, err := os.CreateTemp(dir, ".model-*.tmp")
	if err != nil {
		return "", err
	}
	name := tmp.Name()
	defer os.Remove(name)
	if _, err = tmp.Write(b); err != nil {
		tmp.Close()
		return "", err
	}
	if err = tmp.Close(); err != nil {
		return "", err
	}
	if err = os.Rename(name, path); err != nil {
		return "", err
	}
	return path, nil
}
