package core

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func cadencePreparedFixture(t *testing.T) []Part {
	t.Helper()
	parts := DemoParts()
	path := filepath.Join(t.TempDir(), "source with [unsafe];.step")
	if err := os.WriteFile(path, []byte("ISO-10303-21;\nHEADER;ENDSEC;\nDATA;ENDSEC;END-ISO-10303-21;"), 0600); err != nil {
		t.Fatal(err)
	}
	// This is a labelled source/protocol fixture, not a geometrically valid model.
	parts[0].Model = &Model{Prepared: true, PlacementConvention: "step-baked-rhs-mm-v1", Path: path, X: 999, RotationZ: 42, ExpectedBBoxMM: []float64{-1, -2, 0, 1, 2, 1}}
	return parts
}

func TestCadencePreparedModelHasIdentityMappingAndIndependentReadback(t *testing.T) {
	parts := cadencePreparedFixture(t)
	result, err := ExportCadence(parts, t.TempDir(), "Models", false, true)
	if err != nil {
		t.Fatal(err)
	}
	if len(result.ModelAssociations) != 1 {
		t.Fatal(result.ModelAssociations)
	}
	model := result.ModelAssociations[0]
	if model.Status != "prepared" || model.Verified || model.GeometryVerified || len(model.ModelSHA256) != 64 || len(model.ExpectedBBoxMM) != 6 {
		t.Fatalf("wrong preparation evidence: %+v", model)
	}
	raw, err := os.ReadFile(model.ModelFile)
	if err != nil || !strings.HasPrefix(string(raw), "ISO-10303-21") {
		t.Fatal(err)
	}
	// Portable/prepare mode has a standalone job and never maps while building 2D.
	entry := CadenceEntryNames(parts)[0]
	for _, name := range []string{"build.il", entry + ".step-map.il"} {
		raw, e := os.ReadFile(filepath.Join(result.Directory, "allegro", entry, name))
		if e != nil {
			t.Fatal(e)
		}
		if e = balancedSkill(string(raw)); e != nil {
			t.Fatal(e)
		}
		if strings.Contains(string(raw), "axlStepSet('primary nil") != (name != "build.il") {
			t.Fatalf("wrong preparation/execution split in %s", name)
		}
	}
	plan, err := PrepareCadenceNativeWithModelAssociation(result.Directory, parts, strings.Repeat("c", 64), false, 100, true)
	if err != nil {
		t.Fatal(err)
	}
	job := plan.Allegro[0]
	for _, name := range []string{"build.il", "native-readback.il", "native-compiled.il"} {
		raw, err := os.ReadFile(filepath.Join(result.Directory, filepath.FromSlash(job.Directory), name))
		if err != nil {
			t.Fatal(err)
		}
		text := string(raw)
		if err = balancedSkill(text); err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if strings.Contains(text, "source with") || strings.Contains(text, "999") {
			t.Fatalf("%s leaks unsafe source name or reapplies legacy transform", name)
		}
		if name == "build.il" {
			if !strings.Contains(text, "axlStepSet('primary nil") || !strings.Contains(text, "rotation_z 0.0") || !strings.Contains(text, "offset_x 0.0") {
				t.Fatal("identity mapping absent")
			}
			if strings.Index(text, "axlStepSet('primary nil") > strings.Index(text, "unless(axlSaveDesign(") {
				t.Fatal("mapping happens after save")
			}
		} else if !strings.Contains(text, "PKGDEF_STEP_FILE") || !strings.Contains(text, "PKGDEF_STEP_TRANSFORMATION") {
			t.Fatal("persisted mapping not read")
		}
		if name == "native-compiled.il" && (!strings.Contains(text, "placed->symdef->prop") || !strings.Contains(text, "axlGetAllAttachmentNames")) {
			t.Fatal("PSM model facet readback absent")
		}
	}
}

func TestCadenceModelRejectsUnpreparedAndInvalidSource(t *testing.T) {
	for _, kind := range []string{"unprepared", "invalid-step", "legacy-schema", "empty-schema", "unknown-schema"} {
		t.Run(kind, func(t *testing.T) {
			parts := cadencePreparedFixture(t)
			if kind == "unprepared" {
				parts[0].Model.Prepared = false
			} else if kind == "legacy-schema" {
				parts[0].Model.PlacementConvention = "easyeda-std-kicad-v1"
			} else if kind == "empty-schema" {
				parts[0].Model.PlacementConvention = ""
			} else if kind == "unknown-schema" {
				parts[0].Model.PlacementConvention = "target-specific-guessed-schema"
			} else {
				_ = os.WriteFile(parts[0].Model.Path, []byte("not STEP"), 0600)
			}
			output := t.TempDir()
			if _, err := ExportCadence(parts, output, "BadModel", false, true); err == nil {
				t.Fatal("invalid model was accepted")
			}
			entries, _ := os.ReadDir(output)
			if len(entries) != 0 {
				t.Fatal("failed preparation left partial export")
			}
		})
	}
}
