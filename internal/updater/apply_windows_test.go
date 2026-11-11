//go:build windows

package updater

import (
	"bytes"
	"context"
	"crypto/sha256"
	"debug/pe"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
	"unicode/utf16"

	"golang.org/x/sys/windows"
)

// These are real, independently compiled Windows GUI-subsystem executables. They
// never create a window or call the application: only temporary fixture files are
// read/written. This exercises process waiting, version resources, publication,
// startup acknowledgement and rollback without launching the user's application.
func TestApplyWindowsIntegration(t *testing.T) {
	if runtime.GOARCH != "amd64" {
		t.Skip("the release updater accepts Windows amd64 images")
	}
	oldImage := buildApplyFixture(t, "0.2.0")
	newImage := buildApplyFixture(t, "0.3.0")
	previousTimeout := startupHealthTimeout
	startupHealthTimeout = 2 * time.Second
	t.Cleanup(func() { startupHealthTimeout = previousTimeout })

	for _, tc := range []struct {
		name, mode string
		staleImage bool
		wantError  string
	}{
		{name: "healthy_startup", mode: "healthy"},
		{name: "health_timeout_restores_old", mode: "no-health", wantError: "未通过启动确认"},
		{name: "early_exit_restores_old", mode: "exit", wantError: "未通过启动确认"},
		{name: "staging_failure_restarts_old", mode: "healthy", staleImage: true, wantError: "内嵌版本"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			root := t.TempDir()
			install, data := filepath.Join(root, "installation"), filepath.Join(root, "user-data")
			writeFixture(t, install, "PackageStudio.exe", string(oldImage))
			writeFixture(t, install, "VERSION", "0.2.0\n")
			writeFixture(t, install, "README.md", "old readme")
			writeFixture(t, install, "adapters/altium/fixture.dll", "old adapter")
			preserved := map[string]string{
				filepath.Join(install, "settings.json"):       `{"user":"keep-install-settings"}`,
				filepath.Join(install, "my-work", "part.txt"): "unrelated user file",
				filepath.Join(data, "settings.json"):          `{"user":"keep-data-settings"}`,
				filepath.Join(data, "parts.json"):             `[{"id":"user-part"}]`,
			}
			for path, body := range preserved {
				writeFixture(t, filepath.Dir(path), filepath.Base(path), body)
			}
			writeFixture(t, data, "fixture-mode", tc.mode)
			executable := filepath.Join(install, "PackageStudio.exe")
			// Registered after TempDir so every fixture has exited before Windows
			// attempts to remove its executable and temporary installation.
			t.Cleanup(func() { stopApplyFixtures(t, executable, data) })
			ready, release := filepath.Join(data, "parent-ready"), filepath.Join(data, "parent-release")
			parent := exec.Command(executable, "--fixture-parent", ready, release)
			hideWindow(parent)
			if err := parent.Start(); err != nil {
				t.Fatal(err)
			}
			parentDone := make(chan struct{})
			go func() { _ = parent.Wait(); close(parentDone) }()
			t.Cleanup(func() {
				select {
				case <-parentDone:
				default:
					_ = parent.Process.Kill()
					<-parentDone
				}
			})
			waitApplyFile(t, ready)

			payloadImage := newImage
			if tc.staleImage {
				payloadImage = oldImage // structurally valid, wrong embedded version
			}
			archiveBytes := zipBytes(t, []zipTestEntry{
				{name: "PackageStudio/PackageStudio.exe", data: payloadImage},
				{name: "PackageStudio/VERSION", data: []byte("0.3.0\n")},
				{name: "PackageStudio/README.md", data: []byte("new readme")},
				{name: "PackageStudio/adapters/altium/fixture.dll", data: []byte("new adapter")},
				{name: "PackageStudio/licenses/fixture.txt", data: []byte("new license")},
			})
			cache := filepath.Join(data, "updates")
			archive := filepath.Join(cache, "0.3.0", ArchiveName)
			writeFixture(t, filepath.Dir(archive), filepath.Base(archive), string(archiveBytes))
			manifest := testManifest("0.3.0")
			digest := sha256.Sum256(archiveBytes)
			manifest.SHA256, manifest.Size = hex.EncodeToString(digest[:]), int64(len(archiveBytes))
			writeFixture(t, filepath.Dir(archive), "manifest.json", string(manifestJSON(t, manifest)))
			client := NewClient("0.2.0", cache)
			helper, planPath, err := client.Prepare(archive, executable, data, parent.Process.Pid)
			if err != nil {
				t.Fatal(err)
			}
			if got := []byte(contents(t, helper)); !bytes.Equal(got, oldImage) {
				t.Fatal("prepared updater is not a copy of the installed executable")
			}
			var plan Plan
			if err := json.Unmarshal([]byte(contents(t, planPath)), &plan); err != nil {
				t.Fatal(err)
			}

			// Apply must really wait for this live PackageStudio.exe. A nonexistent
			// PID would bypass the Windows image-path/process-exit checks.
			applyDone := make(chan error, 1)
			go func() { applyDone <- Apply(planPath) }()
			select {
			case err := <-applyDone:
				t.Fatalf("Apply returned before the installed parent exited: %v", err)
			case <-time.After(150 * time.Millisecond):
			}
			writeFixture(t, data, "parent-release", "exit")
			select {
			case err = <-applyDone:
			case <-time.After(15 * time.Second):
				t.Fatal("Apply did not complete")
			}
			<-parentDone
			if tc.wantError == "" && err != nil || tc.wantError != "" && (err == nil || !strings.Contains(err.Error(), tc.wantError)) {
				t.Fatalf("Apply error = %v, want substring %q", err, tc.wantError)
			}

			wantImage, wantVersion, wantAdapter, wantReadme := newImage, "0.3.0\n", "new adapter", "new readme"
			if tc.wantError != "" {
				wantImage, wantVersion, wantAdapter, wantReadme = oldImage, "0.2.0\n", "old adapter", "old readme"
				waitApplyFile(t, filepath.Join(data, "started-old.json"))
				var restarted Health
				readApplyJSON(t, filepath.Join(data, "started-old.json"), &restarted)
				if restarted.Version != "0.2.0" || restarted.PID < 1 {
					t.Fatalf("old application was not restarted: %+v", restarted)
				}
				if _, err := os.Stat(filepath.Join(install, "licenses", "fixture.txt")); !os.IsNotExist(err) {
					t.Fatalf("newly added payload survived rollback: %v", err)
				}
			} else {
				var health, started Health
				readApplyJSON(t, plan.HealthPath, &health)
				readApplyJSON(t, filepath.Join(data, "started-update.json"), &started)
				if health.Version != "0.3.0" || health.Nonce != plan.Nonce || health.PID != started.PID || health.PID < 1 {
					t.Fatalf("invalid startup acknowledgement: %+v, process %+v", health, started)
				}
				if got := contents(t, filepath.Join(install, "licenses", "fixture.txt")); got != "new license" {
					t.Fatal("new payload was not installed")
				}
			}
			if got := []byte(contents(t, executable)); !bytes.Equal(got, wantImage) {
				t.Fatal("installed executable differs from the expected complete image")
			}
			for path, want := range map[string]string{
				filepath.Join(install, "VERSION"):                           wantVersion,
				filepath.Join(install, "README.md"):                         wantReadme,
				filepath.Join(install, "adapters", "altium", "fixture.dll"): wantAdapter,
			} {
				if got := contents(t, path); got != want {
					t.Errorf("%s = %q, want %q", path, got, want)
				}
			}
			for path, want := range preserved {
				if got := contents(t, path); got != want {
					t.Errorf("user file changed: %s", path)
				}
			}
			var result struct {
				Version string `json:"version"`
				Success bool   `json:"success"`
				Error   string `json:"error"`
			}
			readApplyJSON(t, filepath.Join(filepath.Dir(planPath), "installer-result.json"), &result)
			if result.Version != "0.3.0" || result.Success != (tc.wantError == "") || (tc.wantError != "" && result.Error == "") {
				t.Fatalf("incorrect persisted installer result: %+v", result)
			}
		})
	}
}

func readApplyJSON(t *testing.T, path string, dst any) {
	t.Helper()
	if err := json.Unmarshal([]byte(contents(t, path)), dst); err != nil {
		t.Fatal(err)
	}
}

func waitApplyFile(t *testing.T, path string) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if _, err := os.Stat(path); err == nil {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("fixture did not create %s", path)
}

func stopApplyFixtures(t *testing.T, executable, data string) {
	t.Helper()
	_ = os.WriteFile(filepath.Join(data, "fixture-stop"), []byte("stop"), 0600)
	for _, name := range []string{"started-update.json", "started-old.json"} {
		body, err := os.ReadFile(filepath.Join(data, name))
		var process Health
		if err != nil || json.Unmarshal(body, &process) != nil || process.PID < 1 {
			continue
		}
		h, err := windows.OpenProcess(windows.SYNCHRONIZE|windows.PROCESS_QUERY_LIMITED_INFORMATION|windows.PROCESS_TERMINATE, false, uint32(process.PID))
		if err == windows.ERROR_INVALID_PARAMETER {
			continue
		}
		if err != nil {
			t.Errorf("open fixture for cleanup: %v", err)
			continue
		}
		buf, size := make([]uint16, 32768), uint32(32768)
		if err := windows.QueryFullProcessImageName(h, 0, &buf[0], &size); err != nil || !strings.EqualFold(filepath.Clean(windows.UTF16ToString(buf[:size])), filepath.Clean(executable)) {
			// A PID can be reused after the early-exit test. Never stop an
			// unrelated process, even when its PID matches a fixture record.
			_ = windows.CloseHandle(h)
			continue
		}
		result, waitErr := windows.WaitForSingleObject(h, 2000)
		if waitErr == nil && result == uint32(windows.WAIT_TIMEOUT) {
			_ = windows.TerminateProcess(h, 1)
			result, waitErr = windows.WaitForSingleObject(h, 3000)
		}
		_ = windows.CloseHandle(h)
		if waitErr != nil || result != windows.WAIT_OBJECT_0 {
			t.Errorf("fixture did not exit during cleanup: %v, status %d", waitErr, result)
		}
	}
}

func buildApplyFixture(t *testing.T, version string) []byte {
	t.Helper()
	dir := t.TempDir()
	writeFixture(t, dir, "go.mod", "module updaterfixture\n\ngo 1.25.0\n")
	writeFixture(t, dir, "main.go", applyFixtureSource)
	resource := applyVersionResource(t, version)
	if err := os.WriteFile(filepath.Join(dir, "version_windows_amd64.syso"), resource, 0600); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()
	path := filepath.Join(dir, "PackageStudio.exe")
	cmd := exec.CommandContext(ctx, filepath.Join(runtime.GOROOT(), "bin", "go.exe"), "build", "-trimpath", "-ldflags=-H=windowsgui -s -w -X=main.fixtureVersion="+version, "-o", path, ".")
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "GOOS=windows", "GOARCH=amd64", "CGO_ENABLED=0", "GOWORK=off", "GOFLAGS=")
	hideWindow(cmd)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("compile independent GUI fixture: %v\n%s", err, out)
	}
	if err := verifyEmbeddedVersion(path, version); err != nil {
		t.Fatalf("fixture embedded version: %v", err)
	}
	image, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := VerifyPE(image); err != nil {
		t.Fatal(err)
	}
	return image
}

// A minimal COFF resource object avoids depending on an installed resource
// compiler or downloading a third-party generator in CI. The one RT_VERSION
// resource contains the documented 52-byte VS_FIXEDFILEINFO, relocated by Go's
// PE linker. Windows' own version.dll validates it in buildApplyFixture.
func applyVersionResource(t *testing.T, version string) []byte {
	t.Helper()
	v, err := ParseVersion(version)
	if err != nil {
		t.Fatal(err)
	}
	var value bytes.Buffer
	put := func(dst *bytes.Buffer, data any) {
		if err := binary.Write(dst, binary.LittleEndian, data); err != nil {
			t.Fatal(err)
		}
	}
	put(&value, []uint16{0, 52, 0})
	put(&value, utf16.Encode([]rune("VS_VERSION_INFO\x00")))
	for value.Len()%4 != 0 {
		value.WriteByte(0)
	}
	ms, ls := uint32(v[0])<<16|uint32(v[1]), uint32(v[2])<<16
	put(&value, []uint32{0xfeef04bd, 0x00010000, ms, ls, ms, ls, 0x3f, 0, 0x00040004, 1, 0, 0, 0})
	binary.LittleEndian.PutUint16(value.Bytes(), uint16(value.Len()))
	section := make([]byte, 88+value.Len())
	for _, off := range []int{0, 24, 48} {
		binary.LittleEndian.PutUint16(section[off+14:], 1)
	}
	for off, pair := range map[int][2]uint32{16: {16, 0x80000018}, 40: {1, 0x80000030}, 64: {0x0409, 72}, 72: {88, uint32(value.Len())}} {
		binary.LittleEndian.PutUint32(section[off:], pair[0])
		binary.LittleEndian.PutUint32(section[off+4:], pair[1])
	}
	copy(section[88:], value.Bytes())
	var object bytes.Buffer
	put(&object, pe.FileHeader{Machine: pe.IMAGE_FILE_MACHINE_AMD64, NumberOfSections: 1, PointerToSymbolTable: uint32(60 + len(section) + 10), NumberOfSymbols: 1})
	sh := pe.SectionHeader32{SizeOfRawData: uint32(len(section)), PointerToRawData: 60, PointerToRelocations: uint32(60 + len(section)), NumberOfRelocations: 1, Characteristics: pe.IMAGE_SCN_CNT_INITIALIZED_DATA | pe.IMAGE_SCN_MEM_READ}
	copy(sh.Name[:], ".rsrc")
	put(&object, sh)
	object.Write(section)
	put(&object, uint32(72)) // relocation at resource data's RVA
	put(&object, uint32(0))  // section symbol index
	put(&object, uint16(3))  // IMAGE_REL_AMD64_ADDR32NB
	symbol := make([]byte, 18)
	copy(symbol, ".rsrc")
	binary.LittleEndian.PutUint16(symbol[12:], 1) // section number
	symbol[16] = 3                                // IMAGE_SYM_CLASS_STATIC
	object.Write(symbol)
	put(&object, uint32(4)) // empty COFF string table
	return object.Bytes()
}

const applyFixtureSource = `package main

import (
    "encoding/json"
    "os"
    "path/filepath"
    "time"
)

var fixtureVersion string

func exists(path string) bool { _, err := os.Stat(path); return err == nil }
func writeJSON(path string, value any) {
    body, err := json.Marshal(value)
    if err != nil { os.Exit(10) }
    if err = os.WriteFile(path+".tmp", body, 0600); err != nil { os.Exit(11) }
    if err = os.Rename(path+".tmp", path); err != nil { os.Exit(12) }
}
func main() {
    args := os.Args[1:]
    if len(args) == 3 && args[0] == "--fixture-parent" {
        if os.WriteFile(args[1], []byte("ready"), 0600) != nil { os.Exit(13) }
        deadline := time.Now().Add(30*time.Second)
        for time.Now().Before(deadline) && !exists(args[2]) { time.Sleep(10*time.Millisecond) }
        return
    }
    flags := map[string]string{}
    for i:=0; i+1<len(args); i+=2 { flags[args[i]]=args[i+1] }
    data := flags["--data-dir"]
    if data == "" { os.Exit(14) }
    record := map[string]any{"version":fixtureVersion, "pid":os.Getpid(), "nonce":flags["--update-nonce"]}
    health := flags["--update-health"]
    marker := "started-old.json"
    if health != "" { marker = "started-update.json" }
    writeJSON(filepath.Join(data, marker), record)
    mode, _ := os.ReadFile(filepath.Join(data, "fixture-mode"))
    if health != "" {
        if string(mode) == "exit" { return }
        if string(mode) == "healthy" { writeJSON(health, record) }
    }
    deadline := time.Now().Add(30*time.Second)
    for time.Now().Before(deadline) && !exists(filepath.Join(data,"fixture-stop")) { time.Sleep(10*time.Millisecond) }
}
`
