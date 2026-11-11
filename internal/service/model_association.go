package service

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"packagestudio/internal/core"
	"path/filepath"
	"strings"
	"time"
)

// Old caches predate explicit placement metadata. Refresh only when BOTH the
// footprint and the legacy placement are identical. Never silently substitute a
// new footprint/model revision for the part the user has inspected.
func (s *Service) associationSource(ctx context.Context, p core.Part) (core.Model, error) {
	m := *p.Model
	m.Prepared = false
	m.ExpectedBBoxMM = nil
	if m.PlacementConvention != "" {
		return m, nil
	}
	if !lcscPattern.MatchString(p.LCSCID) {
		return m, errors.New("旧模型缺少明确的坐标元数据，请重新获取 C 编号后核对")
	}
	fresh, e := s.fetchPartSource(ctx, p.LCSCID)
	if e != nil {
		return m, fmt.Errorf("补充旧模型元数据失败，请重新获取该 C 编号：%w", e)
	}
	if fresh.Model == nil {
		return m, errors.New("源模型已变化，请重新获取该 C 编号后核对")
	}
	a, b := m, *fresh.Model
	a.Path = ""
	b.Path = ""
	a.PlacementConvention = ""
	b.PlacementConvention = ""
	a.SourceWidthMM = 0
	b.SourceWidthMM = 0
	a.SourceHeightMM = 0
	b.SourceHeightMM = 0
	a.BBoxToleranceMM = 0
	b.BBoxToleranceMM = 0
	aa, _ := json.Marshal(a)
	bb, _ := json.Marshal(b)
	pa, _ := json.Marshal(p.Footprint)
	pb, _ := json.Marshal(fresh.Footprint)
	if !bytes.Equal(aa, bb) || !bytes.Equal(pa, pb) {
		return m, errors.New("源封装或模型位置已更新，请重新获取该 C 编号后核对，未替换当前器件")
	}
	return *fresh.Model, nil
}

func (s *Service) prepareAssociationModel(ctx context.Context, p core.Part) (*core.Model, error) {
	if p.Model == nil {
		return nil, nil
	}
	if !uuidPattern.MatchString(p.Model.UUID) {
		return nil, nil
	}
	ctx, cancel := context.WithTimeout(ctx, 90*time.Second)
	defer cancel()
	model, e := s.associationSource(ctx, p)
	if e != nil {
		return nil, e
	}
	stepPath, e := s.downloadModel(ctx, model.UUID, "step")
	if e != nil {
		return nil, e
	}
	objPath, e := s.downloadModel(ctx, model.UUID, "obj")
	if e != nil {
		return nil, fmt.Errorf("无法获得配套OBJ核对预期尺寸：%w", e)
	}
	step, e := os.ReadFile(stepPath)
	if e != nil {
		return nil, e
	}
	obj, e := os.ReadFile(objPath)
	if e != nil {
		return nil, e
	}
	prepared, e := core.PrepareModelSTEP(step, obj, model)
	if e != nil {
		return nil, e
	}
	if e = ctx.Err(); e != nil {
		return nil, e
	}
	hash := sha256.Sum256(prepared.STEP)
	dir := filepath.Join(s.dataDir, "models", "aligned-v1")
	if e = os.MkdirAll(dir, 0700); e != nil {
		return nil, e
	}
	path := filepath.Join(dir, strings.ToLower(model.UUID)+"-"+hex.EncodeToString(hash[:12])+".step")
	existing, e := os.ReadFile(path)
	if e != nil && !os.IsNotExist(e) {
		return nil, e
	}
	if !bytes.Equal(existing, prepared.STEP) {
		file, e := os.CreateTemp(dir, ".prepared-*.step")
		if e != nil {
			return nil, e
		}
		tmp := file.Name()
		defer os.Remove(tmp)
		_, e = file.Write(prepared.STEP)
		if e == nil {
			e = file.Sync()
		}
		ce := file.Close()
		if e == nil {
			e = ce
		}
		if e != nil {
			return nil, e
		}
		if e = os.Rename(tmp, path); e != nil {
			return nil, e
		}
	}
	model.Path = path
	model.Prepared = true
	model.ExpectedBBoxMM = append([]float64{}, prepared.ExpectedBBoxMM...)
	model.BBoxToleranceMM = .02
	model.PlacementConvention = "step-baked-rhs-mm-v1"
	return &model, nil
}
