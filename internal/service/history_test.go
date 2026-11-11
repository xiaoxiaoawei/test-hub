package service

import (
	"os"
	"path/filepath"
	"testing"
)

func TestHistoryPersistsOutcomesAndClearingPreservesLibraries(t *testing.T) {
	dataDir := t.TempDir()
	s, err := New(dataDir)
	if err != nil {
		t.Fatal(err)
	}
	result, err := s.ExportParts(ExportRequest{PartIDs: []string{"demo-r0603", "demo-r0603"}, OutputDirectory: t.TempDir(), LibraryName: "HistoryTest", IncludeSymbol: true, IncludeFootprint: true, Target: "kicad"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err = s.ExportParts(ExportRequest{Target: "pads", PartIDs: []string{"demo-r0603"}}); err == nil {
		t.Fatal("expected unsupported export to fail")
	}
	reloaded, err := New(dataDir)
	if err != nil {
		t.Fatal(err)
	}
	history := reloaded.GetExportHistory()
	if len(history) != 2 || history[0].Status != "error" || history[0].Error == "" || history[1].Status != "success" {
		t.Fatalf("wrong persisted outcomes: %+v", history)
	}
	if history[1].PartCount != 1 || history[1].Directory != result.Directory || len(history[1].Files) != len(result.Files) {
		t.Fatal("history did not preserve actual exported batch")
	}
	if err = reloaded.ValidateGeneratedFile(result.Files[0]); err != nil {
		t.Fatal(err)
	}
	history[1].Files[0] = "tampered"
	if reloaded.GetExportHistory()[1].Files[0] == "tampered" {
		t.Fatal("history getter exposed mutable state")
	}
	if err = reloaded.ClearExportHistory(); err != nil {
		t.Fatal(err)
	}
	if len(reloaded.GetBootstrap().History) != 0 {
		t.Fatal("history was not cleared")
	}
	if _, err = os.Stat(result.Files[0]); err != nil {
		t.Fatal("clearing history changed generated libraries")
	}
	if err = reloaded.ValidateGeneratedFile(result.Files[0]); err == nil {
		t.Fatal("untracked file was accepted")
	}
	reloaded, err = New(dataDir)
	if err != nil {
		t.Fatal(err)
	}
	if len(reloaded.GetExportHistory()) != 0 {
		t.Fatal("cleared history returned after restart")
	}
}

func TestHistoryWriteFailureDoesNotHideSuccessfulExport(t *testing.T) {
	dataDir := t.TempDir()
	s, err := New(dataDir)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.Mkdir(filepath.Join(dataDir, "history.json"), 0755); err != nil {
		t.Fatal(err)
	}
	result, err := s.ExportParts(ExportRequest{PartIDs: []string{"demo-r0603"}, OutputDirectory: t.TempDir(), LibraryName: "HistoryTest", IncludeSymbol: true, Target: "kicad"})
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Files) == 0 || len(result.Warnings) == 0 {
		t.Fatal("successful export must report history persistence issue")
	}
	if _, err = os.Stat(result.Files[0]); err != nil {
		t.Fatal("exported file is missing")
	}
	if len(s.GetExportHistory()) != 0 {
		t.Fatal("failed history persistence changed memory")
	}
}

func TestLegacySettingsGainUIPreferencesAndNewPreferencesPersist(t *testing.T) {
	dataDir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dataDir, "settings.json"), []byte(`{"theme":"dark","libraryName":"Legacy","includeSymbol":true,"includeFootprint":true}`), 0644); err != nil {
		t.Fatal(err)
	}
	s, err := New(dataDir)
	if err != nil {
		t.Fatal(err)
	}
	settings := s.GetBootstrap().Settings
	if settings.Theme != "dark" || settings.LibraryName != "Legacy" || settings.PreviewLayout != "split" || settings.ExportTarget != "kicad" {
		t.Fatalf("legacy settings not migrated: %+v", settings)
	}
	settings.PreviewLayout = "3d"
	settings.PreviewContent = "symbol"
	settings.ExportTarget = "altium"
	settings.OpenAfterExport = true
	settings.IncludeModel = true
	if _, err = s.SaveSettings(settings); err != nil {
		t.Fatal(err)
	}
	reloaded, err := New(dataDir)
	if err != nil {
		t.Fatal(err)
	}
	got := reloaded.GetBootstrap().Settings
	if got.PreviewLayout != "3d" || got.PreviewContent != "symbol" || got.ExportTarget != "altium" || !got.OpenAfterExport || !got.IncludeModel {
		t.Fatalf("UI preferences not persisted: %+v", got)
	}
}
