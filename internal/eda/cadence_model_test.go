package eda

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"packagestudio/internal/core"
)

func cadenceModelRequest(t *testing.T) Request {
	t.Helper()
	request := cadenceTestRequest(t)
	request.IncludeModel = true
	request.Config.ModelAssociationMode = "native"
	path := filepath.Join(t.TempDir(), "fixture.step")
	if err := os.WriteFile(path, []byte("ISO-10303-21;\nHEADER;ENDSEC;DATA;ENDSEC;END-ISO-10303-21;"), 0600); err != nil {
		t.Fatal(err)
	}
	request.Parts[0].Model = &core.Model{Prepared: true, PlacementConvention: "step-baked-rhs-mm-v1", Path: path, ExpectedBBoxMM: []float64{-1, -2, 0, 1, 2, 1}}
	return request
}

func cadenceModelReport(t *testing.T, spec *cadenceProcessSpec, report *strings.Builder, kind string) error {
	t.Helper()
	if !strings.HasPrefix(spec.Phase, "allegro-") {
		return nil
	}
	batch := filepath.Dir(filepath.Dir(spec.Directory))
	raw, err := os.ReadFile(filepath.Join(batch, "cadence-manifest.json"))
	if err != nil {
		return err
	}
	var manifest core.CadenceManifest
	if err = json.Unmarshal(raw, &manifest); err != nil {
		return err
	}
	var job core.CadenceAllegroJob
	for _, candidate := range manifest.Allegro {
		if candidate.Entry == spec.Entry {
			job = candidate
		}
	}
	if spec.Phase == "allegro-build" {
		if kind == "source-changed" {
			return os.WriteFile(filepath.Join(batch, filepath.FromSlash(job.ModelFile)), []byte("changed STEP"), 0600)
		}
		return nil
	}
	if kind == "missing" {
		return nil
	}
	name := filepath.Base(job.ModelFile)
	transform := "MM,0.0,0.0,0.0,0.0,0.0,0.0"
	attachment := "STEP3D_" + job.Entry
	switch kind {
	case "wrong-name":
		name = "unrelated.step"
	case "wrong-transform":
		transform = "MM,0,0,1,0,0,0"
	case "wrong-units":
		transform = "MILS,0,0,0,0,0,0"
	case "wrong-facet":
		attachment = "STEP3D_another"
	}
	fmt.Fprintf(report, "model-file\t%s,123,456,metadata\nmodel-transform\t%s\n", name, transform)
	if spec.Phase == "allegro-readback-psm" {
		fmt.Fprintf(report, "model-attachment\t%s\n", attachment)
	}
	if kind == "duplicate" {
		fmt.Fprintf(report, "model-file\t%s\n", name)
	}
	return nil
}

func TestCadenceModelNativePublicationIncludesSourceAndReadbackEvidence(t *testing.T) {
	for _, appendMode := range []bool{false, true} {
		t.Run(fmt.Sprint(appendMode), func(t *testing.T) {
			request := cadenceModelRequest(t)
			request.IncludeSymbol = false
			if appendMode {
				request.Config.LibraryMode = "append"
				request.Config.AllegroDirectory = t.TempDir()
			}
			result, err := exportCadence(context.Background(), request, cadenceFakeRunner(t, request, func(spec *cadenceProcessSpec, report *strings.Builder) error {
				return cadenceModelReport(t, spec, report, "")
			}))
			if err != nil {
				t.Fatal(err)
			}
			if len(result.ModelAssociations) != 1 {
				t.Fatal(result.ModelAssociations)
			}
			model := result.ModelAssociations[0]
			if model.Status != "associated" || !model.Verified || model.GeometryVerified || len(model.ActualBBoxMM) != 0 {
				t.Fatalf("incorrect evidence claims: %+v", model)
			}
			sum, err := HashFile(model.ModelFile)
			if err != nil || sum != model.ModelSHA256 {
				t.Fatalf("published source missing/mismatched: %v", err)
			}
			if strings.Contains(model.ModelFile, "stage-") {
				t.Fatal("model mapping points at deleted staging")
			}
			if appendMode && filepath.Dir(model.ModelFile) != request.Config.AllegroDirectory {
				t.Fatal("model not published beside appended PSM")
			}
			// A model-only conflict is rejected before starting any engine. A fresh
			// directory deliberately excludes all pad/DRA/PSM filename conflicts.
			if appendMode {
				request.Config.AllegroDirectory = t.TempDir()
				entries := core.CadenceEntryNames(request.Parts)
				name := entries[0] + "_" + sum[:16] + ".step"
				_ = os.WriteFile(filepath.Join(request.Config.AllegroDirectory, name), []byte("existing user source"), 0600)
				called := false
				_, err = exportCadence(context.Background(), request, cadenceRunnerFunc(func(context.Context, cadenceProcessSpec) (int, error) { called = true; return 0, nil }))
				if err == nil || called || !strings.Contains(err.Error(), "冲突") {
					t.Fatalf("STEP conflict not preflighted: %v called=%v", err, called)
				}
				if !strings.Contains(err.Error(), name) {
					t.Fatalf("did not identify model conflict: %v", err)
				}
			}
		})
	}
}

func TestCadenceModelReadbackFailureNeverPublishesAnyLibrary(t *testing.T) {
	for _, kind := range []string{"missing", "wrong-name", "wrong-transform", "wrong-units", "wrong-facet", "duplicate", "source-changed"} {
		t.Run(kind, func(t *testing.T) {
			request := cadenceModelRequest(t)
			request.Config.LibraryMode = "append"
			request.Config.CaptureLibrary = filepath.Join(t.TempDir(), "existing.olb")
			request.Config.AllegroDirectory = t.TempDir()
			_ = os.WriteFile(request.Config.CaptureLibrary, []byte("old native library"), 0600)
			_, err := exportCadence(context.Background(), request, cadenceFakeRunner(t, request, func(spec *cadenceProcessSpec, report *strings.Builder) error {
				return cadenceModelReport(t, spec, report, kind)
			}))
			if err == nil {
				t.Fatal("invalid association published")
			}
			old, _ := os.ReadFile(request.Config.CaptureLibrary)
			if string(old) != "old native library" {
				t.Fatal("OLB changed despite model association failure")
			}
			entries, _ := os.ReadDir(request.Config.AllegroDirectory)
			if len(entries) != 0 {
				t.Fatal("partial Allegro family was published")
			}
		})
	}
}

func TestCadenceModelCannotAttachToCaptureAlone(t *testing.T) {
	request := cadenceModelRequest(t)
	request.IncludeFootprint = false
	called := false
	_, err := exportCadence(context.Background(), request, cadenceRunnerFunc(func(context.Context, cadenceProcessSpec) (int, error) { called = true; return 0, nil }))
	if err == nil || called || !strings.Contains(err.Error(), "Capture OLB") {
		t.Fatalf("silently downgraded to attachment: %v", err)
	}
}

func TestCadenceModelPrepareModeNeverRunsMappingOrRequiresModelReadback(t *testing.T) {
	for _, mode := range []string{"", "prepare"} {
		t.Run("mode="+mode, func(t *testing.T) {
			request := cadenceModelRequest(t)
			request.Config.ModelAssociationMode = mode
			request.IncludeSymbol = false
			calls := 0
			runner := cadenceFakeRunner(t, request, func(spec *cadenceProcessSpec, _ *strings.Builder) error {
				calls++
				name := "build.il"
				if spec.Phase == "allegro-readback" {
					name = "native-readback.il"
				}
				if spec.Phase == "allegro-readback-psm" {
					name = "native-compiled.il"
				}
				raw, err := os.ReadFile(filepath.Join(spec.Directory, name))
				if err != nil {
					return err
				}
				if strings.Contains(string(raw), "axlStepSet") || strings.Contains(string(raw), "PKGDEF_STEP_FILE") || strings.Contains(string(raw), "axlGetAllAttachmentNames") {
					return fmt.Errorf("prepare mode executes/requires model association in %s", name)
				}
				// Deliberately provide no model records: only the 2D inventory was executed.
				return nil
			})
			result, err := exportCadence(context.Background(), request, runner)
			if err != nil {
				t.Fatal(err)
			}
			if calls != 3 || !result.NativeGenerated || !result.RuntimeVerified {
				t.Fatalf("2D native plan changed: %+v calls=%d", result, calls)
			}
			model := result.ModelAssociations[0]
			if model.Status != "prepared" || model.Verified || model.GeometryVerified || len(model.ActualBBoxMM) != 0 {
				t.Fatalf("prepare falsely marked associated: %+v", model)
			}
			if sum, err := HashFile(model.ModelFile); err != nil || sum != model.ModelSHA256 {
				t.Fatal("prepared STEP was not published beside native drawing", err)
			}
			skill := filepath.Join(filepath.Dir(model.ModelFile), model.Entry+".step-map.il")
			script := filepath.Join(filepath.Dir(model.ModelFile), model.Entry+".step-map.scr")
			raw, err := os.ReadFile(skill)
			if err != nil || !strings.Contains(string(raw), "axlStepSet('primary nil") {
				t.Fatal("standalone prepared mapping job missing", err)
			}
			if _, err = os.Stat(script); err != nil {
				t.Fatal(err)
			}
		})
	}
}
