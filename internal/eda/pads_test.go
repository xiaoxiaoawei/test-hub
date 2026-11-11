package eda

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"packagestudio/internal/core"
)

// These fixtures simulate a local SDK protocol. They are deliberately NOT
// called genuine PADS libraries; real SDK acceptance requires a licensed EDA.
type padsFakeSDK struct {
	calls     []padsJob
	baseline  padsItems
	converted padsEvidence
	alter     func(padsJob, *padsEvidence) error
}

func (f *padsFakeSDK) run(ctx context.Context, job padsJob) (padsEvidence, error) {
	if deadline, ok := ctx.Deadline(); !ok || time.Until(deadline) > 3*time.Minute {
		return padsEvidence{}, errors.New("SDK call had no bounded deadline")
	}
	f.calls = append(f.calls, job)
	e := padsEvidence{Protocol: 1, Nonce: job.Nonce, Phase: job.Phase, Root: job.Root, Engine: "PADS", NativeGenerated: true, RuntimeVerified: job.Phase == "verify", Libraries: []padsLibraryEvidence{}, Warnings: []string{}}
	for _, lib := range job.Libraries {
		before := padsItems{}
		if lib.Existing {
			before = f.baseline
		}
		after := mergePADSItems(before, lib.Expected)
		if job.Phase == "convert" {
			for _, ext := range padsLibraryExtensions {
				if err := os.WriteFile(lib.Base+ext, append([]byte{0, 0x90, 0xff}, []byte("SIMULATED SDK OUTPUT "+ext)...), 0600); err != nil {
					return e, err
				}
			}
		} else {
			for _, prior := range f.converted.Libraries {
				if prior.ID == lib.ID {
					before, after = prior.After, prior.After
				}
			}
		}
		e.Libraries = append(e.Libraries, padsLibraryEvidence{ID: lib.ID, Base: lib.Base, Before: before, After: after, Reopened: job.Phase == "verify"})
	}
	if f.alter != nil {
		if err := f.alter(job, &e); err != nil {
			return e, err
		}
	}
	if job.Phase == "convert" {
		f.converted = e
	}
	return e, nil
}

func padsTestRequest(t *testing.T) Request {
	t.Helper()
	return Request{Parts: core.DemoParts(), OutputDirectory: filepath.Join(t.TempDir(), "output"), LibraryName: "TestParts", IncludeSymbol: true, IncludeFootprint: true, Config: Config{Enabled: true, LibraryMode: "new", PADSRoot: filepath.Join(t.TempDir(), "sdk")}}
}

func padsTestDeps(fake *padsFakeSDK) padsDependencies {
	return padsDependencies{inspect: func(root string, symbol, footprint bool) (padsSDK, error) {
		return padsSDK{Root: root, Hashes: map[string]string{}}, nil
	}, run: fake.run, publish: PublishUpdates}
}

func padsOriginalSet(t *testing.T, base string) map[string]string {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(base), 0755); err != nil {
		t.Fatal(err)
	}
	hashes := map[string]string{}
	for _, ext := range padsLibraryExtensions {
		path := base + ext
		if err := os.WriteFile(path, []byte("ORIGINAL USER LIBRARY "+ext), 0600); err != nil {
			t.Fatal(err)
		}
		hashes[path], _ = HashFile(path)
	}
	return hashes
}

func assertPADSOriginals(t *testing.T, hashes map[string]string) {
	t.Helper()
	for path, before := range hashes {
		after, err := HashFile(path)
		if err != nil || before != after {
			t.Fatalf("original library was changed: %s %v", path, err)
		}
	}
}

func TestPADSNativeTwoIndependentPhasesBeforePublishing(t *testing.T) {
	req := padsTestRequest(t)
	req.Config.LibraryMode = "" // same default as the Cadence integration
	fake := &padsFakeSDK{}
	deps := padsTestDeps(fake)
	published := false
	deps.publish = func(updates []FileUpdate, backup string) ([]string, error) {
		if len(fake.calls) != 2 || fake.calls[0].Phase != "convert" || fake.calls[1].Phase != "verify" {
			t.Fatal("publication preceded independent readback")
		}
		published = true
		return PublishUpdates(updates, backup)
	}
	result, err := exportPADSNative(context.Background(), req, deps)
	if err != nil {
		t.Fatal(err)
	}
	if !published || !result.NativeGenerated || !result.RuntimeVerified || result.ArtifactType != "pads-native-library" || result.ValidationStatus != "native-names-counts-readback" {
		t.Fatalf("wrong native artifact metadata: %+v", result)
	}
	for _, ext := range padsLibraryExtensions {
		if _, err := os.Stat(filepath.Join(result.Directory, "native", "TestParts"+ext)); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := os.Stat(filepath.Join(result.Directory, "native-validation.json")); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(strings.Join(result.Warnings, " "), "真实 EDA") {
		t.Fatal("pending EDA acceptance limitation was lost")
	}
	if len(fake.calls[0].Libraries) != 1 || fake.calls[0].Libraries[0].PartTypeEngine != "logic" {
		t.Fatal("shared library part types must be imported exactly once by Logic")
	}
	if _, err := os.Stat(fake.calls[0].Root); !os.IsNotExist(err) {
		t.Fatal("private SDK workspace was left behind")
	}
}

func TestPADSAppendCopiesFourFilesAndKeepsOriginalsUntilValidated(t *testing.T) {
	req := padsTestRequest(t)
	req.Config.LibraryMode = "append"
	base := filepath.Join(t.TempDir(), "libraries", "Existing")
	hashes := padsOriginalSet(t, base)
	req.Config.PADSSymbolLibrary = base + ".pt9"
	req.Config.PADSFootprintLibrary = base + ".pt9"
	fake := &padsFakeSDK{baseline: padsItems{Parts: []string{"OLD_PART"}, Decals: []string{"OLD_PCB"}, Symbols: []string{"OLD_SCH"}, Drawings: []string{"LOGO"}}}
	fake.alter = func(job padsJob, e *padsEvidence) error {
		assertPADSOriginals(t, hashes)
		raw, _ := json.Marshal(job)
		if strings.Contains(string(raw), strings.ReplaceAll(base, "\\", "\\\\")) {
			t.Fatal("helper received an original destination path")
		}
		return nil
	}
	deps := padsTestDeps(fake)
	result, err := exportPADSNative(context.Background(), req, deps)
	if err != nil {
		t.Fatal(err)
	}
	if len(fake.calls[0].Libraries) != 1 || !fake.calls[0].Libraries[0].Existing || result.BackupDirectory == "" {
		t.Fatalf("append did not stage a shared native set: %+v", result)
	}
	for path, before := range hashes {
		after, _ := HashFile(path)
		if after == before {
			t.Fatal("append was not published")
		}
	}
	backups := 0
	_ = filepath.WalkDir(result.BackupDirectory, func(path string, entry os.DirEntry, err error) error {
		if err == nil && !entry.IsDir() {
			data, _ := os.ReadFile(path)
			if strings.HasPrefix(string(data), "ORIGINAL USER LIBRARY ") {
				backups++
			}
		}
		return err
	})
	if backups != 4 {
		t.Fatalf("missing original backups: %d", backups)
	}
}

func TestPADSNativeRefusesIncompleteSDKAndBadOptionsWithoutRunner(t *testing.T) {
	for _, test := range []struct {
		name   string
		change func(*Request)
	}{
		{"disabled", func(r *Request) { r.Config.Enabled = false }},
		{"mode", func(r *Request) { r.Config.LibraryMode = "replace" }},
		{"relative output", func(r *Request) { r.OutputDirectory = "relative" }},
		{"empty parts", func(r *Request) { r.Parts = nil }},
		{"empty selection", func(r *Request) { r.IncludeSymbol = false; r.IncludeFootprint = false }},
	} {
		t.Run(test.name, func(t *testing.T) {
			req := padsTestRequest(t)
			test.change(&req)
			deps := padsTestDeps(&padsFakeSDK{})
			deps.run = func(context.Context, padsJob) (padsEvidence, error) {
				t.Fatal("invalid request reached SDK")
				return padsEvidence{}, nil
			}
			if _, err := exportPADSNative(context.Background(), req, deps); err == nil {
				t.Fatal("bad option accepted")
			}
		})
	}
	req := padsTestRequest(t)
	if _, err := ExportPADS(context.Background(), req); err == nil {
		t.Fatal("missing real SDK was reported as native success")
	}
	deps := padsTestDeps(&padsFakeSDK{})
	deps.inspect = func(string, bool, bool) (padsSDK, error) { return padsSDK{}, errors.New("SDK absent") }
	if _, err := exportPADSNative(context.Background(), req, deps); err == nil {
		t.Fatal("missing SDK accepted")
	}
	if _, err := os.Stat(req.OutputDirectory); !os.IsNotExist(err) {
		t.Fatal("missing SDK wrote an output directory")
	}
}

func TestPADSAppendRejectsMissingSidecarAndConflict(t *testing.T) {
	for _, mode := range []string{"missing sidecar", "same-name conflict"} {
		t.Run(mode, func(t *testing.T) {
			req := padsTestRequest(t)
			req.Config.LibraryMode = "append"
			base := filepath.Join(t.TempDir(), "Existing")
			hashes := padsOriginalSet(t, base)
			req.Config.PADSSymbolLibrary = base + ".pt9"
			req.Config.PADSFootprintLibrary = base + ".pt9"
			fake := &padsFakeSDK{}
			if mode == "missing sidecar" {
				os.Remove(base + ".ln9")
				delete(hashes, base+".ln9")
			} else {
				fake.baseline = padsItems{Parts: []string{"R_0603_10K"}}
			}
			deps := padsTestDeps(fake)
			deps.publish = func([]FileUpdate, string) ([]string, error) {
				t.Fatal("unsafe append reached publish")
				return nil, nil
			}
			if _, err := exportPADSNative(context.Background(), req, deps); err == nil {
				t.Fatal("unsafe append accepted")
			}
			assertPADSOriginals(t, hashes)
			if mode == "missing sidecar" && len(fake.calls) > 0 {
				t.Fatal("incomplete library reached SDK")
			}
		})
	}
}

func TestPADSNativeRefusesMissingFakeStaleAndFailedReadback(t *testing.T) {
	for _, mode := range []string{"missing output", "renamed ASCII", "empty output", "wrong nonce", "wrong root", "no native flag", "dropped item", "extra item", "readback failure", "not reopened", "changed on reopen"} {
		t.Run(mode, func(t *testing.T) {
			req := padsTestRequest(t)
			fake := &padsFakeSDK{}
			fake.alter = func(job padsJob, e *padsEvidence) error {
				if job.Phase == "convert" {
					switch mode {
					case "missing output":
						return os.Remove(job.Libraries[0].Base + ".pd9")
					case "renamed ASCII":
						return os.WriteFile(job.Libraries[0].Base+".pt9", []byte("*PADS-LIBRARY-PART-TYPES-V9*"), 0600)
					case "empty output":
						return os.WriteFile(job.Libraries[0].Base+".pt9", nil, 0600)
					case "wrong nonce":
						e.Nonce = "stale"
					case "wrong root":
						e.Root = filepath.Dir(job.Root)
					case "no native flag":
						e.NativeGenerated = false
					case "dropped item":
						e.Libraries[0].After.Parts = nil
					case "extra item":
						e.Libraries[0].After.Parts = append(e.Libraries[0].After.Parts, "UNEXPECTED")
					}
				} else {
					switch mode {
					case "readback failure":
						return errors.New("SDK cannot reopen")
					case "not reopened":
						e.Libraries[0].Reopened = false
					case "changed on reopen":
						return os.WriteFile(job.Libraries[0].Base+".pt9", []byte("changed"), 0600)
					}
				}
				return nil
			}
			deps := padsTestDeps(fake)
			deps.publish = func([]FileUpdate, string) ([]string, error) {
				t.Fatal("unverified native output published")
				return nil, nil
			}
			result, err := exportPADSNative(context.Background(), req, deps)
			if err == nil || result.NativeGenerated || result.RuntimeVerified {
				t.Fatalf("invalid SDK output claimed success: %+v %v", result, err)
			}
		})
	}
}

func TestPADSAppendConcurrentModificationAndPublishFailureRetainRecoveryPath(t *testing.T) {
	req := padsTestRequest(t)
	req.Config.LibraryMode = "append"
	base := filepath.Join(t.TempDir(), "Existing")
	padsOriginalSet(t, base)
	req.Config.PADSSymbolLibrary = base + ".pt9"
	req.Config.PADSFootprintLibrary = base + ".pt9"
	fake := &padsFakeSDK{}
	fake.alter = func(job padsJob, e *padsEvidence) error {
		if job.Phase == "verify" {
			return os.WriteFile(base+".pt9", []byte("EXTERNAL EDIT"), 0600)
		}
		return nil
	}
	result, err := exportPADSNative(context.Background(), req, padsTestDeps(fake))
	if err == nil || result.BackupDirectory == "" || !strings.Contains(err.Error(), result.BackupDirectory) {
		t.Fatalf("missing recovery path: %+v %v", result, err)
	}
	actual, _ := os.ReadFile(base + ".pt9")
	if string(actual) != "EXTERNAL EDIT" {
		t.Fatal("concurrent user edit was overwritten")
	}
}

func TestPADSAncestorRedirectRejectedBeforeInspectOrSDK(t *testing.T) {
	outside := t.TempDir()
	link := filepath.Join(t.TempDir(), "linked")
	if err := os.Symlink(outside, link); err != nil {
		if runtime.GOOS != "windows" {
			t.Skip("symlink permission unavailable")
		}
		cmd := exec.Command(filepath.Join(os.Getenv("SystemRoot"), "System32", "cmd.exe"), "/c", "mklink", "/J", link, outside)
		padsHideConsole(cmd)
		if data, e := cmd.CombinedOutput(); e != nil {
			t.Fatalf("could not create owned test junction: %v %s", e, data)
		}
	}
	req := padsTestRequest(t)
	req.OutputDirectory = filepath.Join(link, "output")
	deps := padsTestDeps(&padsFakeSDK{})
	deps.inspect = func(string, bool, bool) (padsSDK, error) {
		t.Fatal("redirected output reached SDK inspection")
		return padsSDK{}, nil
	}
	if _, err := exportPADSNative(context.Background(), req, deps); err == nil {
		t.Fatal("redirected path accepted")
	}
	if _, err := os.Stat(filepath.Join(outside, "output")); !os.IsNotExist(err) {
		t.Fatal("created directory through redirected ancestor")
	}
	base := filepath.Join(outside, "Existing")
	hashes := padsOriginalSet(t, base)
	req = padsTestRequest(t)
	req.Config.LibraryMode = "append"
	req.Config.PADSSymbolLibrary = filepath.Join(link, "Existing.pt9")
	req.Config.PADSFootprintLibrary = req.Config.PADSSymbolLibrary
	if _, err := exportPADSNative(context.Background(), req, deps); err == nil {
		t.Fatal("redirected existing library reached staging")
	}
	assertPADSOriginals(t, hashes)
}

func TestPADSCancelledContextDoesNotStartSDK(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	deps := padsTestDeps(&padsFakeSDK{})
	deps.inspect = func(string, bool, bool) (padsSDK, error) {
		t.Fatal("canceled job inspected SDK")
		return padsSDK{}, nil
	}
	if _, err := exportPADSNative(ctx, padsTestRequest(t), deps); !errors.Is(err, context.Canceled) {
		t.Fatalf("lost cancellation: %v", err)
	}
}

func TestPADSNativeSeparateLibrariesRemainSeparate(t *testing.T) {
	req := padsTestRequest(t)
	req.Config.LibraryMode = "append"
	pcb := filepath.Join(t.TempDir(), "PCB")
	sch := filepath.Join(t.TempDir(), "SCH")
	padsOriginalSet(t, pcb)
	padsOriginalSet(t, sch)
	req.Config.PADSFootprintLibrary = pcb + ".pt9"
	req.Config.PADSSymbolLibrary = sch + ".pt9"
	fake := &padsFakeSDK{}
	if _, err := exportPADSNative(context.Background(), req, padsTestDeps(fake)); err != nil {
		t.Fatal(err)
	}
	if len(fake.calls[0].Libraries) != 2 {
		t.Fatal("separate destination sets were collapsed")
	}
	for _, job := range fake.calls[0].Libraries {
		if job.PartTypeEngine == "layout" && (job.PCB == "" || job.Schematic != "") {
			t.Fatal("wrong Layout import inputs")
		}
		if job.PartTypeEngine == "logic" && (job.Schematic == "" || job.PCB != "") {
			t.Fatal("wrong Logic import inputs")
		}
	}
}

func TestPADSHelperProtocolRejectsMalformedOversizedAndTrailingReports(t *testing.T) {
	for _, raw := range []string{`not-json`, `{"unexpected":true}`, `{} {}`, strings.Repeat(" ", 16*1024*1024+1)} {
		if _, err := parsePADSEvidence([]byte(raw)); err == nil {
			t.Fatal("bad helper protocol accepted")
		}
	}
	raw, _ := json.Marshal(padsEvidence{Protocol: 1, Nonce: "test", Engine: "PADS"})
	parsed, err := parsePADSEvidence(append([]byte{0xef, 0xbb, 0xbf}, raw...))
	if err != nil || parsed.Engine != "PADS" || parsed.Protocol != 1 {
		t.Fatalf("valid UTF8 helper protocol rejected: %+v %v", parsed, err)
	}
}

func TestPADSFailedEngineKeepsOnlyBoundedDiagnosticText(t *testing.T) {
	req := padsTestRequest(t)
	fake := &padsFakeSDK{}
	fake.alter = func(job padsJob, e *padsEvidence) error {
		for name, value := range map[string]string{"inventory.txt": "ERROR|test SDK failure", "private.exe": "SDK executable", "private.ini": "user preferences"} {
			if err := os.WriteFile(filepath.Join(job.Root, name), []byte(value), 0600); err != nil {
				return err
			}
		}
		return errors.New("simulated SDK failure")
	}
	result, err := exportPADSNative(context.Background(), req, padsTestDeps(fake))
	if err == nil || result.Directory == "" || !strings.Contains(err.Error(), result.Directory) || result.NativeGenerated || result.RuntimeVerified {
		t.Fatalf("failed attempt lost diagnostics or claimed success: %+v %v", result, err)
	}
	if _, e := os.Stat(filepath.Join(result.Directory, "inventory.txt")); e != nil {
		t.Fatal("failure inventory was deleted")
	}
	for _, file := range result.Files {
		ext := strings.ToLower(filepath.Ext(file))
		if ext == ".exe" || ext == ".ini" || ext == ".pt9" || ext == ".pd9" || ext == ".ld9" || ext == ".ln9" {
			t.Fatalf("private SDK/native copy leaked into diagnostics: %s", file)
		}
	}
	if _, e := os.Stat(fake.calls[0].Root); !os.IsNotExist(e) {
		t.Fatal("temporary SDK workspace remained after diagnostic preservation")
	}
}
