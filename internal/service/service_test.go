package service

import (
	"context"
	"io"
	"net/http"
	"os"
	"packagestudio/internal/core"
	"path/filepath"
	"strings"
	"testing"
)

type transportFunc func(*http.Request) (*http.Response, error)

func (f transportFunc) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestNewExportTargetsPersistAndDispatch(t *testing.T) {
	dir := t.TempDir()
	s, err := New(filepath.Join(dir, "data"))
	if err != nil {
		t.Fatal(err)
	}
	for _, target := range []string{"pads", "cadence"} {
		boot := s.GetBootstrap()
		if !boot.Capabilities[target] {
			t.Fatalf("missing %s capability", target)
		}
		settings := boot.Settings
		settings.ExportTarget = target
		if _, err = s.SaveSettings(settings); err != nil {
			t.Fatal(err)
		}
		reloaded, err := New(s.dataDir)
		if err != nil {
			t.Fatal(err)
		}
		if reloaded.GetBootstrap().Settings.ExportTarget != target {
			t.Fatal("export target lost on restart")
		}
		part := core.DemoParts()[0]
		result, err := s.ExportParts(ExportRequest{PartIDs: []string{part.ID}, OutputDirectory: filepath.Join(dir, "export"), LibraryName: "Dispatch", IncludeSymbol: true, IncludeFootprint: true, Target: target})
		if err != nil {
			t.Fatalf("%s dispatch failed: %v", target, err)
		}
		if len(result.Files) == 0 {
			t.Fatal("export produced no files")
		}
		for _, file := range result.Files {
			if _, err := os.Stat(file); err != nil {
				t.Fatal(err)
			}
		}
	}
	if len(s.GetExportHistory()) != 2 {
		t.Fatal("new targets not recorded in history")
	}
}

func TestInvalidPartIDNeverRequestsNetwork(t *testing.T) {
	s, err := New(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	s.client = &http.Client{Transport: transportFunc(func(r *http.Request) (*http.Response, error) {
		t.Fatal("invalid input reached network")
		return nil, nil
	})}
	for _, id := range []string{"", "../../x", "C123?foo=bar", "https://example.com", "C1"} {
		if _, err = s.FetchPart(context.Background(), id); err == nil {
			t.Errorf("accepted invalid ID %q", id)
		}
	}
}
func TestPublicFetchPersistsParsedPartAndSettings(t *testing.T) {
	dir := t.TempDir()
	s, err := New(dir)
	if err != nil {
		t.Fatal(err)
	}
	fixture, err := os.ReadFile(filepath.Join("..", "core", "testdata", "C25804.json"))
	if err != nil {
		t.Fatal(err)
	}
	s.client = &http.Client{Transport: transportFunc(func(r *http.Request) (*http.Response, error) {
		if r.URL.Host != "easyeda.com" || !strings.Contains(r.URL.Path, "C25804") {
			t.Fatalf("unexpected endpoint %s", r.URL)
		}
		return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader(string(fixture))), Header: make(http.Header)}, nil
	})}
	p, err := s.FetchPart(context.Background(), " c25804 ")
	if err != nil {
		t.Fatal(err)
	}
	if len(p.Footprint.Pads) != 2 || len(p.Symbol.Pins) != 2 {
		t.Fatalf("unexpected geometry: %+v", p)
	}
	settings := s.GetBootstrap().Settings
	settings.Theme = "dark"
	settings.LibraryName = "UnitTestParts"
	if _, err = s.SaveSettings(settings); err != nil {
		t.Fatal(err)
	}
	reloaded, err := New(dir)
	if err != nil {
		t.Fatal(err)
	}
	if reloaded.GetBootstrap().Settings.Theme != "dark" {
		t.Fatal("settings were not persisted")
	}
	if _, ok := reloaded.parts[p.ID]; !ok {
		t.Fatal("fetched part was not persisted")
	}
}
func TestExportRejectsUnsupportedTargetAndRelativeDirectory(t *testing.T) {
	s, err := New(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if _, err = s.ExportParts(ExportRequest{Target: "pads"}); err == nil {
		t.Fatal("unsupported target was accepted")
	}
	if _, err = s.ExportParts(ExportRequest{OutputDirectory: "relative", PartIDs: []string{"demo-r0603"}}); err == nil {
		t.Fatal("relative output directory was accepted")
	}
}

func TestFailedCacheWriteKeepsMemoryUnchanged(t *testing.T) {
	dir := t.TempDir()
	s, err := New(dir)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.Mkdir(filepath.Join(dir, "parts.json"), 0755); err != nil {
		t.Fatal(err)
	}
	p := core.DemoParts()[0]
	p.ID = "new-part"
	if err = s.put(p); err == nil {
		t.Fatal("expected cache write failure")
	}
	if _, ok := s.parts[p.ID]; ok {
		t.Fatal("failed write changed in-memory cache")
	}
	if err = s.RemovePart("demo-r0603"); err == nil {
		t.Fatal("expected remove persistence failure")
	}
	if _, ok := s.parts["demo-r0603"]; !ok {
		t.Fatal("failed remove changed in-memory cache")
	}
}

func TestSymbolOnlyExportDoesNotDownloadModel(t *testing.T) {
	s, err := New(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	p := s.parts["demo-r0603"]
	p.Model = &core.Model{UUID: strings.Repeat("a", 32)}
	s.parts[p.ID] = p
	s.client = &http.Client{Transport: transportFunc(func(r *http.Request) (*http.Response, error) {
		t.Fatal("symbol-only export downloaded model")
		return nil, nil
	})}
	res, err := s.ExportParts(ExportRequest{Target: "kicad", PartIDs: []string{p.ID}, OutputDirectory: t.TempDir(), LibraryName: "Symbols", IncludeSymbol: true, IncludeModel: true})
	if err != nil {
		t.Fatal(err)
	}
	if len(res.Files) < 2 {
		t.Fatal("symbol output missing")
	}
}
