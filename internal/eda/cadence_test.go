package eda

import (
	"context"
	"debug/pe"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"packagestudio/internal/core"
)

type cadenceRunnerFunc func(context.Context, cadenceProcessSpec) (int, error)

func (f cadenceRunnerFunc) Run(ctx context.Context, spec cadenceProcessSpec) (int, error) {
	return f(ctx, spec)
}

// This is PE metadata for a never-executed fakeRunner fixture, not an EDA binary.
func cadenceTestEngine(t *testing.T) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "Capture.exe")
	data := make([]byte, 192)
	copy(data, "MZ")
	binary.LittleEndian.PutUint32(data[60:], 128)
	copy(data[128:], "PE\x00\x00")
	binary.LittleEndian.PutUint16(data[132:], pe.IMAGE_FILE_MACHINE_AMD64)
	binary.LittleEndian.PutUint16(data[150:], pe.IMAGE_FILE_EXECUTABLE_IMAGE)
	if err := os.WriteFile(path, data, 0600); err != nil {
		t.Fatal(err)
	}
	return path
}
func cadenceTestRequest(t *testing.T) Request {
	t.Helper()
	exe := cadenceTestEngine(t)
	allegro := filepath.Join(filepath.Dir(exe), "Allegro.exe")
	data, err := os.ReadFile(exe)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(allegro, data, 0600); err != nil {
		t.Fatal(err)
	}
	return Request{Parts: core.DemoParts(), OutputDirectory: t.TempDir(), LibraryName: "Native Test", IncludeSymbol: true, IncludeFootprint: true, Config: Config{Enabled: true, LibraryMode: "new", CaptureExe: exe, AllegroExe: allegro}}
}

func cadenceFakeRunner(t *testing.T, request Request, change func(*cadenceProcessSpec, *strings.Builder) error) cadenceRunner {
	t.Helper()
	entries := core.CadenceEntryNames(request.Parts)
	return cadenceRunnerFunc(func(ctx context.Context, spec cadenceProcessSpec) (int, error) {
		if err := ctx.Err(); err != nil {
			return 0, err
		}
		// No user destination is handed to the engine: every working path is a
		// private staging descendant, including append's copied existing OLB.
		if !strings.Contains(spec.Directory, ".cadence-native-stage-") {
			t.Fatalf("engine received user destination: %s", spec.Directory)
		}
		if strings.HasSuffix(spec.Phase, "-build") {
			batch := filepath.Dir(spec.Directory)
			if strings.HasPrefix(spec.Phase, "allegro") {
				batch = filepath.Dir(batch)
			}
			raw, e := os.ReadFile(filepath.Join(batch, "cadence-manifest.json"))
			if e != nil {
				return 0, e
			}
			var manifest core.CadenceManifest
			if e = json.Unmarshal(raw, &manifest); e != nil {
				return 0, e
			}
			files := []string{}
			if strings.HasPrefix(spec.Phase, "capture") {
				files = append(files, manifest.CaptureLibrary)
			} else {
				for _, job := range manifest.Allegro {
					if job.Entry == spec.Entry {
						files = append(files, job.ExpectedFiles...)
					}
				}
			}
			for _, rel := range files {
				path := filepath.Join(batch, filepath.FromSlash(rel))
				previous, _ := os.ReadFile(path)
				if request.Config.LibraryMode == "append" && strings.HasSuffix(path, ".olb") && string(previous) != "old native library" {
					t.Fatalf("append did not give engine a private OLB copy: %q", previous)
				}
				if e = os.WriteFile(path, append(previous, []byte("\nFAKE-RUNNER-NATIVE:"+filepath.Base(path))...), 0600); e != nil {
					return 0, e
				}
			}
		}
		phase := strings.TrimPrefix(strings.TrimPrefix(spec.Phase, "capture-"), "allegro-")
		var report strings.Builder
		fmt.Fprintf(&report, "protocol\tpackage-studio.cadence/1\nphase\t%s\nnonce\t%s\npid\t42\nengine\tfixture\n", phase, spec.Nonce)
		if spec.Phase == "capture-readback" {
			preserved := 0
			if request.Config.LibraryMode == "append" {
				preserved = 1
			}
			fmt.Fprintf(&report, "preserved\t%d\n", preserved)
			for i, part := range request.Parts {
				fmt.Fprintf(&report, "package\t%s\t%s\n", hex.EncodeToString([]byte(entries[i])), hex.EncodeToString([]byte(spec.Nonce)))
				for _, pin := range part.Symbol.Pins {
					unit := pin.Unit
					if unit < 1 {
						unit = 1
					}
					fmt.Fprintf(&report, "pin\t%s\t%d\t%s\t%s\n", hex.EncodeToString([]byte(entries[i])), unit, hex.EncodeToString([]byte(pin.Number)), hex.EncodeToString([]byte(pin.Name)))
				}
			}
		}
		if strings.HasPrefix(spec.Phase, "allegro-") && phase != "build" {
			fmt.Fprintf(&report, "entry\t%s\n", spec.Entry)
			for i, part := range request.Parts {
				if entries[i] == spec.Entry {
					for _, pin := range part.Footprint.Pads {
						fmt.Fprintf(&report, "pin\t%s\n", pin.Number)
					}
				}
			}
		}
		if change != nil {
			if e := change(&spec, &report); e != nil {
				return 0, e
			}
		}
		report.WriteString("complete\t1\n")
		if err := os.WriteFile(spec.ReportPath+".tmp", []byte(report.String()), 0600); err != nil {
			return 0, err
		}
		if err := os.Rename(spec.ReportPath+".tmp", spec.ReportPath); err != nil {
			return 0, err
		}
		return 42, nil
	})
}

func TestCadenceNativeProtocolPublishesOnlyReadbackVerifiedFiles(t *testing.T) {
	request := cadenceTestRequest(t)
	calls := 0
	runner := cadenceFakeRunner(t, request, func(_ *cadenceProcessSpec, _ *strings.Builder) error { calls++; return nil })
	result, err := exportCadence(context.Background(), request, runner)
	if err != nil {
		t.Fatal(err)
	}
	if calls != 5 || !result.NativeGenerated || !result.RuntimeVerified || result.ArtifactType != "native" || result.ValidationStatus != "engine-readback-verified" {
		t.Fatalf("wrong validated result: calls=%d %+v", calls, result)
	}
	native := 0
	for _, file := range result.Files {
		if _, err = os.Stat(file); err != nil {
			t.Fatal(err)
		}
		switch filepath.Ext(file) {
		case ".olb", ".dra", ".pad", ".psm":
			native++
		}
	}
	if native != 4 {
		t.Fatalf("expected four native fixture files, got %d", native)
	}
	if _, err = os.Stat(filepath.Join(result.BackupDirectory, "manifest.json")); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(strings.Join(result.Warnings, " "), "本开发机没有Cadence") {
		t.Fatal("fakeRunner must not erase real-runtime qualification")
	}
}

func TestCadenceNativeAppendStagesExistingLibraryAndRejectsConflicts(t *testing.T) {
	request := cadenceTestRequest(t)
	request.Config.LibraryMode = "append"
	request.Config.CaptureLibrary = filepath.Join(t.TempDir(), "Existing.olb")
	request.Config.AllegroDirectory = t.TempDir()
	if err := os.WriteFile(request.Config.CaptureLibrary, []byte("old native library"), 0600); err != nil {
		t.Fatal(err)
	}
	result, err := exportCadence(context.Background(), request, cadenceFakeRunner(t, request, nil))
	if err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(request.Config.CaptureLibrary)
	if err != nil || !strings.HasPrefix(string(data), "old native library\nFAKE-RUNNER") {
		t.Fatalf("append result: %q %v", data, err)
	}
	backups, _ := filepath.Glob(filepath.Join(result.BackupDirectory, "*-Existing.olb"))
	if len(backups) != 1 {
		t.Fatalf("original OLB backup absent: %v", backups)
	}
	data, _ = os.ReadFile(backups[0])
	if string(data) != "old native library" {
		t.Fatal("backup changed")
	}
	called := false
	_, err = exportCadence(context.Background(), request, cadenceRunnerFunc(func(context.Context, cadenceProcessSpec) (int, error) { called = true; return 0, nil }))
	if err == nil || !strings.Contains(err.Error(), "冲突") || called {
		t.Fatalf("existing Allegro file must fail before engine: %v called=%v", err, called)
	}
}

func TestCadenceNativeRejectsInvalidReadbackWithoutPublishing(t *testing.T) {
	for _, kind := range []string{"wrong-nonce", "wrong-pid", "wrong-pins", "wrong-compiled", "engine-error"} {
		t.Run(kind, func(t *testing.T) {
			request := cadenceTestRequest(t)
			request.Config.LibraryMode = "append"
			request.Config.CaptureLibrary = filepath.Join(t.TempDir(), "Existing.olb")
			request.Config.AllegroDirectory = t.TempDir()
			_ = os.WriteFile(request.Config.CaptureLibrary, []byte("old native library"), 0600)
			runner := cadenceFakeRunner(t, request, func(spec *cadenceProcessSpec, report *strings.Builder) error {
				if kind == "engine-error" {
					_ = os.WriteFile(filepath.Join(spec.Directory, "fixture.log"), []byte("diagnostic failure"), 0600)
					return errors.New("fixture engine cannot read corrupted native library")
				}
				if kind == "wrong-nonce" {
					text := strings.ReplaceAll(report.String(), spec.Nonce, strings.Repeat("b", 64))
					report.Reset()
					report.WriteString(text)
				}
				if kind == "wrong-pid" {
					text := strings.ReplaceAll(report.String(), "pid\t42", "pid\t1")
					report.Reset()
					report.WriteString(text)
				}
				if kind == "wrong-pins" && spec.Phase == "capture-readback" {
					report.WriteString("pin\t626164\t1\t39\t39\n")
				}
				if kind == "wrong-compiled" && spec.Phase == "allegro-readback-psm" {
					report.WriteString("pin\textra\n")
				}
				return nil
			})
			result, err := exportCadence(context.Background(), request, runner)
			if err == nil || result.NativeGenerated || result.RuntimeVerified {
				t.Fatalf("invalid engine report was accepted: %+v %v", result, err)
			}
			data, _ := os.ReadFile(request.Config.CaptureLibrary)
			if string(data) != "old native library" {
				t.Fatal("failure changed original OLB")
			}
			entries, _ := os.ReadDir(request.Config.AllegroDirectory)
			if len(entries) != 0 {
				t.Fatal("failed readback published partial Allegro files")
			}
			if !strings.Contains(err.Error(), "CadenceFailed_") {
				t.Fatalf("diagnostic directory missing from error: %v", err)
			}
			failed, _ := filepath.Glob(filepath.Join(request.OutputDirectory, "CadenceFailed_*"))
			if len(failed) != 1 {
				t.Fatalf("expected retained diagnostic batch: %v", failed)
			}
			_ = filepath.WalkDir(failed[0], func(path string, e os.DirEntry, err error) error {
				if err != nil {
					t.Fatal(err)
				}
				if filepath.Ext(path) == ".olb" {
					t.Fatal("original OLB copied to diagnostics")
				}
				return nil
			})
		})
	}
}

func TestCadenceNativePreflightMissingCorruptAndCancelled(t *testing.T) {
	request := cadenceTestRequest(t)
	request.Config.CaptureExe = filepath.Join(t.TempDir(), "Capture.exe")
	runner := cadenceRunnerFunc(func(context.Context, cadenceProcessSpec) (int, error) {
		t.Fatal("preflight must not start an engine")
		return 0, nil
	})
	if _, err := exportCadence(context.Background(), request, runner); err == nil {
		t.Fatal("missing engine accepted")
	}
	_ = os.WriteFile(request.Config.CaptureExe, []byte("not a PE"), 0600)
	if _, err := exportCadence(context.Background(), request, runner); err == nil {
		t.Fatal("corrupt engine accepted")
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := exportCadence(ctx, request, runner); !errors.Is(err, context.Canceled) {
		t.Fatalf("cancellation lost: %v", err)
	}
	entries, _ := os.ReadDir(request.OutputDirectory)
	if len(entries) != 0 {
		t.Fatal("preflight left a partial artifact")
	}
}

func TestCadenceAppendDetectsExternalChangeBeforePublication(t *testing.T) {
	request := cadenceTestRequest(t)
	request.IncludeFootprint = false
	request.Config.LibraryMode = "append"
	request.Config.CaptureLibrary = filepath.Join(t.TempDir(), "Existing.olb")
	_ = os.WriteFile(request.Config.CaptureLibrary, []byte("old native library"), 0600)
	runner := cadenceFakeRunner(t, request, func(spec *cadenceProcessSpec, _ *strings.Builder) error {
		if spec.Phase == "capture-readback" {
			return os.WriteFile(request.Config.CaptureLibrary, []byte("external user change"), 0600)
		}
		return nil
	})
	result, err := exportCadence(context.Background(), request, runner)
	if err == nil || result.NativeGenerated {
		t.Fatalf("concurrent library change was overwritten: %v %+v", err, result)
	}
	data, _ := os.ReadFile(request.Config.CaptureLibrary)
	if string(data) != "external user change" {
		t.Fatal("concurrent user change lost")
	}
}

func TestCadenceReportPresenceIsNotValidation(t *testing.T) {
	path := filepath.Join(t.TempDir(), "report.tsv")
	for _, data := range []string{"done", "protocol\tpackage-studio.cadence/1\nphase\treadback\nnonce\tbad\ncomplete\t1\n", "protocol\tpackage-studio.cadence/1\nphase\treadback\nnonce\t" + strings.Repeat("a", 64) + "\n"} {
		_ = os.WriteFile(path, []byte(data), 0600)
		if _, err := readCadenceReport(path, "readback", strings.Repeat("a", 64)); err == nil {
			t.Fatal("marker or incomplete report accepted")
		}
	}
}

func TestCadenceWrongExecutableNameRejectedBeforeStart(t *testing.T) {
	for _, tc := range []struct{ product, name string }{{"Capture", "cmd.exe"}, {"Allegro", "cmd.exe"}, {"Capture", "Allegro.exe"}, {"Allegro", "Capture.exe"}} {
		t.Run(tc.product+"-"+tc.name, func(t *testing.T) {
			request := cadenceTestRequest(t)
			data, err := os.ReadFile(request.Config.CaptureExe)
			if err != nil {
				t.Fatal(err)
			}
			wrong := filepath.Join(t.TempDir(), tc.name)
			if err = os.WriteFile(wrong, data, 0600); err != nil {
				t.Fatal(err)
			}
			if tc.product == "Capture" {
				request.Config.CaptureExe = wrong
			} else {
				request.Config.AllegroExe = wrong
			}
			runner := cadenceRunnerFunc(func(context.Context, cadenceProcessSpec) (int, error) {
				t.Fatal("wrong application must be rejected before process start")
				return 0, nil
			})
			if _, err = exportCadence(context.Background(), request, runner); err == nil || !strings.Contains(err.Error(), "文件名不匹配") {
				t.Fatalf("wrong executable accepted: %v", err)
			}
			entries, _ := os.ReadDir(request.OutputDirectory)
			if len(entries) != 0 {
				t.Fatal("wrong executable created staging output")
			}
		})
	}
	data, err := os.ReadFile(cadenceTestEngine(t))
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct{ product, name string }{{"Capture", "CAPTURE.EXE"}, {"Allegro", "pcb_editor.exe"}} {
		path := filepath.Join(t.TempDir(), tc.name)
		if err = os.WriteFile(path, data, 0600); err != nil {
			t.Fatal(err)
		}
		if err = cadenceCheckEngine(path, tc.product); err != nil {
			t.Fatalf("known executable spelling rejected: %v", err)
		}
	}
}

func TestCadenceDiscoveryReadyIsPEOnlyNotRuntimeQualification(t *testing.T) {
	root := t.TempDir()
	bin := filepath.Join(root, "tools", "bin")
	if err := os.MkdirAll(bin, 0700); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(cadenceTestEngine(t))
	if err != nil {
		t.Fatal(err)
	}
	valid := filepath.Join(bin, "allegro.exe")
	invalid := filepath.Join(bin, "Capture.exe")
	_ = os.WriteFile(valid, data, 0600)
	_ = os.WriteFile(invalid, []byte("invalid PE"), 0600)
	t.Setenv("CDSROOT", root)
	seenValid, seenInvalid := false, false
	for _, candidate := range DetectCadence() {
		if candidate.Executable == valid {
			seenValid = true
			if !candidate.Ready || candidate.RuntimeVerified || candidate.Architecture != "x64" {
				t.Fatalf("PE discovery qualification wrong: %+v", candidate)
			}
		}
		if candidate.Executable == invalid {
			seenInvalid = true
			if candidate.Ready || candidate.RuntimeVerified {
				t.Fatal("corrupt candidate marked ready")
			}
		}
	}
	if !seenValid || !seenInvalid {
		t.Fatalf("discovery omitted candidates valid=%v invalid=%v", seenValid, seenInvalid)
	}
}
