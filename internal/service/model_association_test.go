package service

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"packagestudio/internal/core"
	"packagestudio/internal/eda"
	"path/filepath"
	"strings"
	"testing"
)

func associationFixture(t *testing.T) (*Service, core.Part) {
	t.Helper()
	s, e := New(t.TempDir())
	if e != nil {
		t.Fatal(e)
	}
	p := s.parts["demo-r0603"]
	p.Model = &core.Model{UUID: strings.Repeat("b", 32), PlacementConvention: "easyeda-std-kicad-v1", RotationZ: -37, X: 1.3, Y: -2.4, Z: -.7}
	s.parts[p.ID] = p
	dir := filepath.Join(s.dataDir, "models")
	if e = os.MkdirAll(dir, 0700); e != nil {
		t.Fatal(e)
	}
	for _, ext := range []string{"step", "obj"} {
		data, e := os.ReadFile(filepath.Join("..", "core", "testdata", "model_asymmetric."+ext))
		if e != nil {
			t.Fatal(e)
		}
		if e = os.WriteFile(filepath.Join(dir, p.Model.UUID+"."+ext), data, 0600); e != nil {
			t.Fatal(e)
		}
	}
	s.client = &http.Client{Transport: transportFunc(func(*http.Request) (*http.Response, error) {
		t.Fatal("prepared fixture unexpectedly requested network")
		return nil, nil
	})}
	return s, p
}
func TestModelAssociationPreparePipelineAndCacheIsolation(t *testing.T) {
	s, p := associationFixture(t)
	for _, target := range []string{"pads", "cadence"} {
		result, e := s.ExportParts(ExportRequest{PartIDs: []string{p.ID}, Target: target, OutputDirectory: t.TempDir(), LibraryName: "ModelTest", IncludeSymbol: true, IncludeFootprint: true, IncludeModel: true, NativeConfig: eda.Config{ModelAssociationMode: "prepare"}})
		if e != nil {
			t.Fatal(target, e)
		}
		if len(result.ModelAssociations) != 1 {
			t.Fatalf("%s missing model results %+v", target, result)
		}
		a := result.ModelAssociations[0]
		if a.Status != "prepared" || a.Verified || a.GeometryVerified || a.ModelSHA256 == "" || len(a.ExpectedBBoxMM) != 6 {
			t.Fatalf("misleading model status %+v", a)
		}
		file := a.ModelFile
		if !filepath.IsAbs(file) {
			file = filepath.Join(result.Directory, filepath.FromSlash(file))
		}
		if _, e = os.Stat(file); e != nil {
			t.Fatal("model path invalid", file, e)
		}
		h := s.GetExportHistory()
		h[0].ModelAssociations[0].ExpectedBBoxMM[0] = 999
		if s.GetExportHistory()[0].ModelAssociations[0].ExpectedBBoxMM[0] == 999 {
			t.Fatal("history model bounds shared mutable slices")
		}
	}
	if p := s.parts[p.ID]; p.Model.Prepared || p.Model.Path != "" || len(p.Model.ExpectedBBoxMM) != 0 {
		t.Fatal("export mutated cached source model")
	}
}
func TestModelAssociationUnsupportedNeverPublishes(t *testing.T) {
	s, p := associationFixture(t)
	p.Model.RotationX = 90
	s.parts[p.ID] = p
	output := filepath.Join(t.TempDir(), "exports")
	_, e := s.ExportParts(ExportRequest{PartIDs: []string{p.ID}, Target: "pads", OutputDirectory: output, IncludeFootprint: true, IncludeModel: true})
	if e == nil || !strings.Contains(e.Error(), "X/Y") {
		t.Fatal(e)
	}
	if _, e = os.Stat(output); !os.IsNotExist(e) {
		t.Fatal("unsupported model published output")
	}
}
func TestLegacyAssociationMetadataOnlyRefreshedForIdenticalGeometry(t *testing.T) {
	raw, e := os.ReadFile(filepath.Join("..", "core", "testdata", "C25804.json"))
	if e != nil {
		t.Fatal(e)
	}
	fresh, e := core.ParseEasyEDA(raw, "C25804")
	if e != nil {
		t.Fatal(e)
	}
	s, e := New(t.TempDir())
	if e != nil {
		t.Fatal(e)
	}
	s.client = &http.Client{Transport: transportFunc(func(*http.Request) (*http.Response, error) {
		return &http.Response{StatusCode: 200, Header: make(http.Header), Body: io.NopCloser(bytes.NewReader(raw))}, nil
	})}
	old := fresh
	m := *old.Model
	m.PlacementConvention = ""
	m.SourceWidthMM = 0
	m.SourceHeightMM = 0
	old.Model = &m
	model, e := s.associationSource(context.Background(), old)
	if e != nil || model.PlacementConvention != "easyeda-std-kicad-v1" {
		t.Fatal(model, e)
	}
	source, _ := json.Marshal(old)
	old.Model.RotationZ = 123
	if _, e = s.associationSource(context.Background(), old); e == nil {
		t.Fatal("silently replaced inspected placement")
	}
	if e = json.Unmarshal(source, &old); e != nil {
		t.Fatal(e)
	}
	old.Footprint.Pads[0].X += .2
	if _, e = s.associationSource(context.Background(), old); e == nil {
		t.Fatal("silently replaced inspected footprint")
	}
}
