package service

import (
	"context"
	"os"
	"packagestudio/internal/eda"
	"path/filepath"
	"testing"
)

func TestNativeMissingEngineNeverFallsBack(t *testing.T) {
	root := t.TempDir()
	s, e := New(filepath.Join(root, "data"))
	if e != nil {
		t.Fatal(e)
	}
	old := filepath.Join(root, "existing.olb")
	if e = os.WriteFile(old, []byte("user-library"), 0600); e != nil {
		t.Fatal(e)
	}
	for _, target := range []string{"pads", "cadence"} {
		cfg := eda.Config{Enabled: true, LibraryMode: "append", PADSRoot: filepath.Join(root, "absent"), PADSSymbolLibrary: old, PADSFootprintLibrary: old, CaptureExe: filepath.Join(root, "absent-Capture.exe"), AllegroExe: filepath.Join(root, "absent-allegro.exe"), CaptureLibrary: old, AllegroDirectory: root}
		result, e := s.ExportParts(ExportRequest{PartIDs: []string{"demo-r0603"}, Target: target, OutputDirectory: filepath.Join(root, "exports"), LibraryName: "missing", IncludeSymbol: true, IncludeFootprint: true, NativeConfig: cfg})
		if e == nil || result.NativeGenerated || result.RuntimeVerified {
			t.Fatalf("missing engine accepted: %+v %v", result, e)
		}
		b, e := os.ReadFile(old)
		if e != nil || string(b) != "user-library" {
			t.Fatal("existing library modified")
		}
	}
	for _, entry := range s.GetExportHistory() {
		if entry.Status != "error" || entry.NativeGenerated {
			t.Fatalf("misleading history: %+v", entry)
		}
	}
}
func TestNativeSettingsAndOldDefaults(t *testing.T) {
	root := t.TempDir()
	s, e := New(root)
	if e != nil {
		t.Fatal(e)
	}
	cfg := s.GetBootstrap().Settings
	if cfg.NativeConfig.Enabled || cfg.NativeConfig.LibraryMode != "new" {
		t.Fatal("native enabled by default")
	}
	cfg.NativeConfig = eda.Config{Enabled: true, LibraryMode: "append", PADSRoot: "selected", CaptureLibrary: "library"}
	if _, e = s.SaveSettings(cfg); e != nil {
		t.Fatal(e)
	}
	s, e = New(root)
	if e != nil {
		t.Fatal(e)
	}
	got := s.GetBootstrap().Settings.NativeConfig
	if got != cfg.NativeConfig {
		t.Fatalf("lost config: %+v", got)
	}
}
func TestCancelledExportDoesNotCreateLibrary(t *testing.T) {
	root := t.TempDir()
	s, e := New(filepath.Join(root, "data"))
	if e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	out := filepath.Join(root, "exports")
	_, e = s.ExportPartsContext(ctx, ExportRequest{PartIDs: []string{"demo-r0603"}, Target: "kicad", OutputDirectory: out, IncludeSymbol: true})
	if e == nil {
		t.Fatal("cancelled request accepted")
	}
	if _, e = os.Stat(out); !os.IsNotExist(e) {
		t.Fatal("cancelled export created output")
	}
}
