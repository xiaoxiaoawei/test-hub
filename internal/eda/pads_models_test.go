package eda

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"packagestudio/internal/core"
)

// Simulated SDK outputs exercise the publication boundary, not the proprietary
// CAD format. They must never be described as genuine PADS model files.
type padsModelFake struct {
	calls []padsModelJob
	alter func(padsModelJob, *padsModelEvidence) error
}

func (f *padsModelFake) run(ctx context.Context, j padsModelJob) (padsModelEvidence, error) {
	if deadline, ok := ctx.Deadline(); !ok || time.Until(deadline) > 10*time.Minute {
		return padsModelEvidence{}, errors.New("unbounded model SDK invocation")
	}
	if e := ctx.Err(); e != nil {
		return padsModelEvidence{}, e
	}
	f.calls = append(f.calls, j)
	report := padsModelEvidence{Protocol: 1, Nonce: j.Nonce, Root: j.Root, Phase: j.Phase, Reuse: j.Reuse, Version: "2019.2.7", Items: []padsModelReadback{}}
	for _, item := range j.Items {
		name := item.ModelName + ".step"
		path := filepath.Join(j.Reuse, "3DModels", "PartDB", "Packages", "User", name, "SAT", name+"_1.xtd")
		if j.Phase == "convert" && !item.Existing {
			if e := os.MkdirAll(filepath.Dir(path), 0700); e != nil {
				return report, e
			}
			if e := os.WriteFile(path, []byte("\x00SIMULATED MODEL "+name), 0600); e != nil {
				return report, e
			}
		}
		hash, e := HashFile(path)
		if e != nil {
			return report, e
		}
		report.Items = append(report.Items, padsModelReadback{PartID: item.PartID, Entry: item.Entry, Decal: item.Decal, ModelName: name, Mapped: true, Local: j.Phase == "convert" && !item.Existing, BBoxMM: append([]float64{}, item.ExpectedBBoxMM...), PackageSHA256: hash, LoadedSHA256: hash, FreshDocument: true})
	}
	if j.Phase == "convert" {
		path := filepath.Join(j.Reuse, "3DModels", "mapping.dat")
		f, e := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
		if e != nil {
			return report, e
		}
		for _, i := range j.Items {
			if !i.Existing {
				_, e = f.WriteString(i.Entry + "\n")
			}
		}
		f.Close()
		if e != nil {
			return report, e
		}
	}
	if f.alter != nil {
		if e := f.alter(j, &report); e != nil {
			return report, e
		}
	}
	return report, nil
}
func padsModelRequest(t *testing.T, suffix string) (Request, core.ExportResult) {
	t.Helper()
	root := t.TempDir()
	part := core.DemoParts()[0]
	part.ID = "part-" + suffix
	part.Name = "MODEL_" + suffix
	model := filepath.Join(root, "prepared.step")
	if e := os.WriteFile(model, []byte("ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;"), 0600); e != nil {
		t.Fatal(e)
	}
	part.Model = &core.Model{Path: model, Prepared: true, PlacementConvention: "step-baked-rhs-mm-v1", ExpectedBBoxMM: []float64{-1, -2, 0, 1, 2, 1}, BBoxToleranceMM: .05, ScaleX: 1, ScaleY: 1, ScaleZ: 1}
	req := Request{Parts: []core.Part{part}, OutputDirectory: filepath.Join(root, "output"), LibraryName: "Models", IncludeSymbol: true, IncludeFootprint: true, IncludeModel: true, Config: Config{Enabled: true, ModelAssociationMode: "native", PADSModelExe: filepath.Join(root, "sdk", "Programs", "powerpcb.exe"), PADSReuseDirectory: filepath.Join(root, "Reuse")}}
	ascii, e := core.ExportPADS(req.Parts, req.OutputDirectory, req.LibraryName, true, true)
	if e != nil {
		t.Fatal(e)
	}
	return req, ascii
}
func padsModelTestDeps(f *padsModelFake) padsModelDependencies {
	return padsModelDependencies{inspect: func(s string) (padsModelSDK, error) { return padsModelSDK{Executable: s, Version: "2019.2.7"}, nil }, run: f.run, publish: PublishUpdates}
}

func TestPADSModelsPrepareIsPortableAndNeverClaimsNativeAssociation(t *testing.T) {
	req, ascii := padsModelRequest(t, "A")
	req.Config.ModelAssociationMode = "prepare"
	deps := padsModelDependencies{inspect: func(string) (padsModelSDK, error) { t.Fatal("prepare inspected SDK"); return padsModelSDK{}, nil }, publish: PublishUpdates}
	out, e := exportPADSModels(context.Background(), req, ascii, deps)
	if e != nil {
		t.Fatal(e)
	}
	if out.NativeGenerated || out.RuntimeVerified || len(out.ModelAssociations) != 1 || out.ModelAssociations[0].Status != "prepared" || out.ModelAssociations[0].Verified || out.ModelAssociations[0].GeometryVerified {
		t.Fatalf("prepared package overstated verification: %+v", out)
	}
	if _, e = os.Stat(req.Config.PADSReuseDirectory); !os.IsNotExist(e) {
		t.Fatal("prepare touched native Reuse")
	}
	found := false
	for _, path := range out.Files {
		if filepath.Base(path) == "association-plan.json" {
			raw, _ := os.ReadFile(path)
			if strings.Contains(string(raw), ".packagestudio-") {
				t.Fatal("portable plan retained temporary paths")
			}
			var p struct {
				Items []padsModelItem `json:"items"`
			}
			if e = json.Unmarshal(raw, &p); e != nil {
				t.Fatal(e)
			}
			if p.Items[0].STEP != "models/"+p.Items[0].ModelName+".step" {
				t.Fatal("plan model path is not relative")
			}
			found = true
		}
	}
	if !found {
		t.Fatal("association plan missing")
	}
}

func TestPADSModelsFreshThenManagedAppendReadbacksAllMappings(t *testing.T) {
	req, ascii := padsModelRequest(t, "A")
	req.Config.Enabled = false // ASCII + modern Reuse must not require the 9.4 engine.
	if e := os.MkdirAll(req.Config.PADSReuseDirectory, 0700); e != nil {
		t.Fatal(e)
	}
	user := filepath.Join(req.Config.PADSReuseDirectory, "user-notes.txt")
	os.WriteFile(user, []byte("USER DATA"), 0600)
	empty := filepath.Join(req.Config.PADSReuseDirectory, ".keep")
	os.WriteFile(empty, nil, 0600)
	f := &padsModelFake{}
	first, e := exportPADSModels(context.Background(), req, ascii, padsModelTestDeps(f))
	if e != nil {
		t.Fatal(e)
	}
	if len(f.calls) != 2 || !first.ModelAssociations[0].Verified || !first.ModelAssociations[0].GeometryVerified || first.ModelAssociations[0].Status != "associated" {
		t.Fatalf("SDK readback or per-model status missing: %+v", first)
	}
	if first.NativeGenerated || first.RuntimeVerified {
		t.Fatal("3D association changed native 2D library metadata")
	}
	before, e := padsModelTree(req.Config.PADSReuseDirectory)
	if e != nil {
		t.Fatal(e)
	}
	next, batch := padsModelRequest(t, "B")
	next.Config.PADSReuseDirectory = req.Config.PADSReuseDirectory
	f2 := &padsModelFake{}
	second, e := exportPADSModels(context.Background(), next, batch, padsModelTestDeps(f2))
	if e != nil {
		t.Fatal(e)
	}
	if len(f2.calls) != 2 || len(f2.calls[1].Items) != 2 || !f2.calls[0].Items[0].Existing || f2.calls[0].Items[1].Existing {
		t.Fatalf("append did not verify old and new mappings: %+v", f2.calls)
	}
	after, e := padsModelTree(req.Config.PADSReuseDirectory)
	if e != nil {
		t.Fatal(e)
	}
	if data, e := os.ReadFile(user); e != nil || string(data) != "USER DATA" {
		t.Fatal("user file changed")
	}
	if st, e := os.Stat(empty); e != nil || st.Size() != 0 {
		t.Fatal("empty user file changed")
	}
	for path, hash := range before {
		if strings.HasSuffix(path, ".xtd") && after[path] != hash {
			t.Fatal("append changed old XTD")
		}
	}
	manifest, e := padsReadReuseManifest(req.Config.PADSReuseDirectory, after)
	if e != nil || len(manifest.Items) != 2 {
		t.Fatalf("managed manifest invalid: %v %+v", e, manifest)
	}
	if second.BackupDirectory == "" {
		t.Fatal("append did not retain backup path")
	}
	if _, e = os.Stat(filepath.Join(second.BackupDirectory, "manifest.json")); e != nil {
		t.Fatal("transaction backup metadata missing", e)
	}
}

func TestPADSModelsAppendVerifiesOldGeometryAndPreservesOriginalOnFailure(t *testing.T) {
	req, ascii := padsModelRequest(t, "A")
	if _, e := exportPADSModels(context.Background(), req, ascii, padsModelTestDeps(&padsModelFake{})); e != nil {
		t.Fatal(e)
	}
	before, _ := padsModelTree(req.Config.PADSReuseDirectory)
	next, batch := padsModelRequest(t, "B")
	next.Config.PADSReuseDirectory = req.Config.PADSReuseDirectory
	f := &padsModelFake{alter: func(j padsModelJob, e *padsModelEvidence) error {
		if j.Phase == "verify" {
			for i := range e.Items {
				if j.Items[i].Existing {
					e.Items[i].BBoxMM[5] += .2
				}
			}
		}
		return nil
	}}
	if _, e := exportPADSModels(context.Background(), next, batch, padsModelTestDeps(f)); e == nil {
		t.Fatal("append did not verify previous mapping geometry")
	}
	after, _ := padsModelTree(req.Config.PADSReuseDirectory)
	if !padsHashesEqual(before, after) {
		t.Fatal("failed old-mapping readback changed original Reuse")
	}
}

func TestPADSModelsFailureNeverPublishesOrClaimsAssociation(t *testing.T) {
	cases := map[string]func(padsModelJob, *padsModelEvidence) error{
		"native centering": func(j padsModelJob, e *padsModelEvidence) error { e.Items[0].BBoxMM[0] += .5; return nil },
		"local cache": func(j padsModelJob, e *padsModelEvidence) error {
			if j.Phase == "verify" {
				e.Items[0].Local = true
			}
			return nil
		},
		"hash mismatch": func(j padsModelJob, e *padsModelEvidence) error {
			if j.Phase == "verify" {
				e.Items[0].LoadedSHA256 = strings.Repeat("0", 64)
			}
			return nil
		},
		"stale nonce":     func(j padsModelJob, e *padsModelEvidence) error { e.Nonce = strings.Repeat("b", 48); return nil },
		"not fresh":       func(j padsModelJob, e *padsModelEvidence) error { e.Items[0].FreshDocument = false; return nil },
		"missing mapping": func(j padsModelJob, e *padsModelEvidence) error { e.Items = nil; return nil },
		"reopen modifies reuse": func(j padsModelJob, e *padsModelEvidence) error {
			if j.Phase == "verify" {
				return os.WriteFile(filepath.Join(j.Reuse, "3DModels", "mapping.dat"), []byte("changed"), 0600)
			}
			return nil
		},
		"empty native XTD": func(j padsModelJob, e *padsModelEvidence) error {
			if j.Phase == "verify" {
				i := j.Items[0]
				name := i.ModelName + ".step"
				path := filepath.Join(j.Reuse, "3DModels", "PartDB", "Packages", "User", name, "SAT", name+"_1.xtd")
				if err := os.WriteFile(path, nil, 0600); err != nil {
					return err
				}
				h, _ := HashFile(path)
				e.Items[0].PackageSHA256 = h
				e.Items[0].LoadedSHA256 = h
			}
			return nil
		},
	}
	for name, alter := range cases {
		t.Run(name, func(t *testing.T) {
			req, ascii := padsModelRequest(t, "A")
			f := &padsModelFake{alter: alter}
			out, e := exportPADSModels(context.Background(), req, ascii, padsModelTestDeps(f))
			if e == nil {
				t.Fatal("invalid SDK evidence accepted")
			}
			if _, e = os.Stat(req.Config.PADSReuseDirectory); !os.IsNotExist(e) {
				t.Fatal("failed SDK validation changed target Reuse")
			}
			for _, a := range out.ModelAssociations {
				if a.Verified || a.GeometryVerified || a.Status == "associated" {
					t.Fatal("failed attempt retained success metadata")
				}
			}
		})
	}
}

func TestPADSModelsRefusesUnknownModifiedAndConflictingReuse(t *testing.T) {
	for _, scenario := range []string{"unknown", "externally modified", "conflict"} {
		t.Run(scenario, func(t *testing.T) {
			req, ascii := padsModelRequest(t, "A")
			if scenario == "unknown" {
				path := filepath.Join(req.Config.PADSReuseDirectory, "3DModels", "foreign.dat")
				os.MkdirAll(filepath.Dir(path), 0700)
				os.WriteFile(path, []byte("FOREIGN"), 0600)
			} else {
				if _, e := exportPADSModels(context.Background(), req, ascii, padsModelTestDeps(&padsModelFake{})); e != nil {
					t.Fatal(e)
				}
				if scenario == "externally modified" {
					os.WriteFile(filepath.Join(req.Config.PADSReuseDirectory, "3DModels", "mapping.dat"), []byte("EXTERNAL"), 0600)
				}
			}
			before, _ := padsModelTree(req.Config.PADSReuseDirectory)
			f := &padsModelFake{}
			_, e := exportPADSModels(context.Background(), req, ascii, padsModelTestDeps(f))
			if e == nil {
				t.Fatal("unproven or conflicting merge accepted")
			}
			after, _ := padsModelTree(req.Config.PADSReuseDirectory)
			if !padsHashesEqual(before, after) || len(f.calls) != 0 {
				t.Fatal("rejected merge called SDK or changed user's Reuse")
			}
		})
	}
}

func TestPADSModelsPublishFailureClearsVerifiedStatusAndRetainsRecoveryPath(t *testing.T) {
	req, ascii := padsModelRequest(t, "A")
	deps := padsModelTestDeps(&padsModelFake{})
	deps.publish = func([]FileUpdate, string) ([]string, error) { return nil, errors.New("simulated transaction failure") }
	out, e := exportPADSModels(context.Background(), req, ascii, deps)
	if e == nil || out.BackupDirectory == "" || !strings.Contains(e.Error(), out.BackupDirectory) {
		t.Fatal("publish failure lost recovery path", e)
	}
	if out.ModelAssociations[0].Verified || out.ModelAssociations[0].GeometryVerified || out.ModelAssociations[0].Status == "associated" {
		t.Fatal("unpublished association reported as verified")
	}
}

func TestPADSModelsRejectUnpreparedAndCombinedNativeBeforeEngines(t *testing.T) {
	req, ascii := padsModelRequest(t, "A")
	req.Parts[0].Model.Prepared = false
	f := &padsModelFake{}
	if _, e := exportPADSModels(context.Background(), req, ascii, padsModelTestDeps(f)); e == nil || len(f.calls) != 0 {
		t.Fatal("unprepared STEP reached SDK")
	}
	called := false
	deps := padsTestDeps(&padsFakeSDK{})
	deps.inspect = func(string, bool, bool) (padsSDK, error) { called = true; return padsSDK{}, nil }
	if _, e := exportPADSNative(context.Background(), req, deps); e == nil || called {
		t.Fatal("combined 2D/3D native publication was not rejected early")
	}
}

func TestPADSModelEvidenceStrictParsing(t *testing.T) {
	for _, data := range []string{`{"protocol":1,"unexpected":true}`, `{} {}`, `{"protocol":"wrong"}`} {
		if _, e := parsePADSModelEvidence([]byte(data)); e == nil {
			t.Fatalf("accepted %s", data)
		}
	}
}

func TestPADSModelMergedASCIIHasOneTerminalEndAfterBothParts(t *testing.T) {
	root := t.TempDir()
	old := filepath.Join(root, "old.d")
	next := filepath.Join(root, "new.d")
	os.WriteFile(old, []byte("*PADS-LIBRARY-PCB-DECALS-V9*\n\nOLD_NAME record\n*END*\n"), 0600)
	os.WriteFile(next, []byte("*PADS-LIBRARY-PCB-DECALS-V9*\n\nNEW_NAME record\n*END*\n"), 0600)
	if e := padsMergeModelASCII(old, next); e != nil {
		t.Fatal(e)
	}
	data, _ := os.ReadFile(next)
	text := string(data)
	if strings.Count(text, "*END*") != 1 || strings.Count(text, "*PADS-LIBRARY-") != 1 || strings.Index(text, "NEW_NAME") > strings.Index(text, "*END*") || !strings.Contains(text, "OLD_NAME") {
		t.Fatal("ASCII append ended before new entry", text)
	}
}

func TestPADSNativeLibraryPreparesModelsInsideFinalTransaction(t *testing.T) {
	req, _ := padsModelRequest(t, "A")
	req.Config.LibraryMode = "new"
	req.Config.ModelAssociationMode = "prepare"
	f := &padsFakeSDK{}
	deps := padsTestDeps(f)
	publishes := 0
	deps.publish = func(updates []FileUpdate, backup string) ([]string, error) {
		publishes++
		return PublishUpdates(updates, backup)
	}
	out, e := exportPADSNative(context.Background(), req, deps)
	if e != nil {
		t.Fatal(e)
	}
	if publishes != 1 || len(out.ModelAssociations) != 1 || out.ModelAssociations[0].Status != "prepared" || out.ModelAssociations[0].Verified {
		t.Fatalf("native prepared status/transaction invalid: %+v", out)
	}
	model := out.ModelAssociations[0].ModelFile
	if !strings.Contains(model, string(filepath.Separator)+"interchange"+string(filepath.Separator)) || strings.Contains(model, ".packagestudio-") {
		t.Fatal("model path was not relocated", model)
	}
	if _, e = os.Stat(model); e != nil {
		t.Fatal("prepared model not published", e)
	}
	for _, path := range out.Files {
		if filepath.Base(path) == "associations.json" {
			raw, _ := os.ReadFile(path)
			if strings.Contains(string(raw), ".packagestudio-") {
				t.Fatal("published associations refer to staging")
			}
			var a []core.ModelAssociation
			if e = json.Unmarshal(raw, &a); e != nil || a[0].ModelFile != model {
				t.Fatal("persisted association relocation mismatch", e)
			}
		}
	}
}

func TestPADSNativeLibraryModelPreparationFailureNeverPublishes(t *testing.T) {
	req, _ := padsModelRequest(t, "A")
	req.Config.LibraryMode = "new"
	req.Config.ModelAssociationMode = "prepare"
	req.Parts[0].Model.BBoxToleranceMM = 5
	deps := padsTestDeps(&padsFakeSDK{})
	published := false
	deps.publish = func([]FileUpdate, string) ([]string, error) { published = true; return nil, nil }
	if _, e := exportPADSNative(context.Background(), req, deps); e == nil || published {
		t.Fatal("2D library published before unsuccessful model preparation")
	}
}
