package updater

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"debug/pe"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func testManifest(version string) Manifest {
	return Manifest{SchemaVersion: 1, Product: "package-studio", Version: version, DownloadURL: Repository + "/releases/download/package-studio-v" + version + "/" + ArchiveName, NotesURL: Repository + "/releases/tag/package-studio-v" + version, SHA256: strings.Repeat("a", 64), Size: 1024, PublishedAt: "2026-11-11T12:00:00Z", ReleaseNotes: "Test release"}
}
func manifestJSON(t *testing.T, m Manifest) []byte {
	t.Helper()
	b, e := json.Marshal(m)
	if e != nil {
		t.Fatal(e)
	}
	return b
}
func TestVersionsAreStrictAndComparedNumerically(t *testing.T) {
	for _, bad := range []string{"", "v1.2.3", "1.2", "1.2.3.4", "01.2.3", "1.02.3", "-1.2.3", "1.2.3-beta", "1.2.3+build", " 1.2.3", "1.2.3\n", "65536.0.0", "999999999999999999999.0.0"} {
		if _, e := ParseVersion(bad); e == nil {
			t.Errorf("accepted version %q", bad)
		}
	}
	for _, c := range []struct {
		latest, current string
		want            bool
	}{{"2.10.0", "2.9.99", true}, {"1.0.0", "0.65535.65535", true}, {"1.2.3", "1.2.3", false}, {"1.2.2", "1.2.3", false}, {"0.1.0", "1.0.0", false}} {
		got, e := Newer(c.latest, c.current)
		if e != nil || got != c.want {
			t.Fatalf("%s newer than %s: %v %v", c.latest, c.current, got, e)
		}
	}
}
func TestManifestRejectsWrongRepositoryAndAmbiguousJSON(t *testing.T) {
	m := testManifest("0.3.0")
	if _, e := ParseManifest(manifestJSON(t, m)); e != nil {
		t.Fatal(e)
	}
	for _, bad := range []string{strings.Replace(m.DownloadURL, "https:", "http:", 1), strings.Replace(m.DownloadURL, "xiaoxiaoawei", "somebody", 1), strings.Replace(m.DownloadURL, "test-hub/", "test-hub.evil/", 1), m.DownloadURL + "?redirect=1", m.DownloadURL + "#other", strings.Replace(m.DownloadURL, "github.com", "github.com.attacker.invalid", 1), strings.Replace(m.DownloadURL, "package-studio-v0.3.0", "package-studio-v0.3.1", 1)} {
		badManifest := m
		badManifest.DownloadURL = bad
		if badManifest.Validate() == nil {
			t.Errorf("accepted release URL %s", bad)
		}
	}
	body := string(manifestJSON(t, m))
	for _, bad := range []string{strings.TrimSuffix(body, "}") + `,"version":"0.3.0"}`, strings.TrimSuffix(body, "}") + `,"Version":"0.3.0"}`, strings.TrimSuffix(body, "}") + `,"ver\u0073ion":"0.3.0"}`, strings.TrimSuffix(body, "}") + `,"unknown":true}`, body + `{}`, body + `null`, `[]`, `null`} {
		if _, e := ParseManifest([]byte(bad)); e == nil {
			t.Errorf("accepted ambiguous JSON %s", bad)
		}
	}
	for name, change := range map[string]func(*Manifest){"schema": func(m *Manifest) { m.SchemaVersion = 2 }, "product": func(m *Manifest) { m.Product = "other" }, "notes-repository": func(m *Manifest) { m.NotesURL = "https://github.com/other/repo" }, "short-hash": func(m *Manifest) { m.SHA256 = "abc" }, "upper-hash": func(m *Manifest) { m.SHA256 = strings.Repeat("A", 64) }, "too-small": func(m *Manifest) { m.Size = 511 }, "too-large": func(m *Manifest) { m.Size = MaxArchiveBytes + 1 }, "date": func(m *Manifest) { m.PublishedAt = "yesterday" }, "notes": func(m *Manifest) { m.ReleaseNotes = strings.Repeat("x", 16001) }} {
		t.Run(name, func(t *testing.T) {
			bad := m
			change(&bad)
			if _, e := ParseManifest(manifestJSON(t, bad)); e == nil {
				t.Fatal("invalid manifest accepted")
			}
		})
	}
}

// A non-executed minimal PE32+ GUI image gives archive tests a deterministic
// fixture. It has a real .text section/entry byte, but no Windows version resource.
func minimalGUIPE(t *testing.T) []byte {
	t.Helper()
	header := make([]byte, 128)
	copy(header, "MZ")
	binary.LittleEndian.PutUint32(header[60:], 128)
	var b bytes.Buffer
	b.Write(header)
	b.WriteString("PE\x00\x00")
	fh := pe.FileHeader{Machine: pe.IMAGE_FILE_MACHINE_AMD64, NumberOfSections: 1, SizeOfOptionalHeader: 240, Characteristics: pe.IMAGE_FILE_EXECUTABLE_IMAGE | pe.IMAGE_FILE_LARGE_ADDRESS_AWARE}
	oh := pe.OptionalHeader64{Magic: 0x20b, AddressOfEntryPoint: 0x1000, BaseOfCode: 0x1000, ImageBase: 0x140000000, SectionAlignment: 0x1000, FileAlignment: 0x200, SizeOfImage: 0x2000, SizeOfHeaders: 0x200, Subsystem: pe.IMAGE_SUBSYSTEM_WINDOWS_GUI, NumberOfRvaAndSizes: 16}
	sh := pe.SectionHeader32{VirtualSize: 1, VirtualAddress: 0x1000, SizeOfRawData: 0x200, PointerToRawData: 0x200, Characteristics: pe.IMAGE_SCN_CNT_CODE | pe.IMAGE_SCN_MEM_EXECUTE | pe.IMAGE_SCN_MEM_READ}
	copy(sh.Name[:], ".text")
	for _, v := range []any{fh, oh, sh} {
		if e := binary.Write(&b, binary.LittleEndian, v); e != nil {
			t.Fatal(e)
		}
	}
	for b.Len() < 0x400 {
		b.WriteByte(0)
	}
	out := b.Bytes()
	out[0x200] = 0xc3
	return out
}

type zipTestEntry struct {
	name string
	data []byte
	mode os.FileMode
}

func normalEntries(t *testing.T) []zipTestEntry {
	return []zipTestEntry{{name: "PackageStudio/PackageStudio.exe", data: minimalGUIPE(t)}, {name: "PackageStudio/VERSION", data: []byte("0.3.0\n")}, {name: "PackageStudio/README.md", data: []byte("Official application readme")}, {name: "PackageStudio/adapters/altium/test.dll", data: []byte("ADAPTER-CHECKSUM-UNIQUE-PAYLOAD")}}
}
func zipBytes(t *testing.T, entries []zipTestEntry) []byte {
	t.Helper()
	var b bytes.Buffer
	w := zip.NewWriter(&b)
	for _, entry := range entries {
		h := &zip.FileHeader{Name: entry.name, Method: zip.Store}
		if entry.mode != 0 {
			h.SetMode(entry.mode)
			if entry.mode&os.ModeSymlink != 0 && entry.mode&os.ModeDir != 0 {
				h.CreatorVersion = 3 << 8
				h.ExternalAttrs = 0120777 << 16
			}
		} else {
			h.SetMode(0644)
		}
		f, e := w.CreateHeader(h)
		if e != nil {
			t.Fatal(e)
		}
		if _, e = f.Write(entry.data); e != nil {
			t.Fatal(e)
		}
	}
	if e := w.Close(); e != nil {
		t.Fatal(e)
	}
	return b.Bytes()
}
func savedArchive(t *testing.T, raw []byte) (string, Manifest) {
	t.Helper()
	file := filepath.Join(t.TempDir(), ArchiveName)
	if e := os.WriteFile(file, raw, 0600); e != nil {
		t.Fatal(e)
	}
	sum := sha256.Sum256(raw)
	m := testManifest("0.3.0")
	m.Size = int64(len(raw))
	m.SHA256 = hex.EncodeToString(sum[:])
	return file, m
}
func TestPERejectsWrongArchitectureConsoleDLLAndInvalidEntry(t *testing.T) {
	valid := minimalGUIPE(t)
	if e := VerifyPE(valid); e != nil {
		t.Fatal(e)
	}
	for name, mutate := range map[string]func([]byte){"x86": func(b []byte) { binary.LittleEndian.PutUint16(b[132:], pe.IMAGE_FILE_MACHINE_I386) }, "console": func(b []byte) { binary.LittleEndian.PutUint16(b[128+24+68:], pe.IMAGE_SUBSYSTEM_WINDOWS_CUI) }, "dll": func(b []byte) {
		binary.LittleEndian.PutUint16(b[128+22:], pe.IMAGE_FILE_EXECUTABLE_IMAGE|pe.IMAGE_FILE_DLL)
	}, "not-executable": func(b []byte) { binary.LittleEndian.PutUint16(b[128+22:], 0) }, "no-entry": func(b []byte) { binary.LittleEndian.PutUint32(b[128+24+16:], 0) }, "outside-entry": func(b []byte) { binary.LittleEndian.PutUint32(b[128+24+16:], 0x5000) }, "non-code": func(b []byte) { binary.LittleEndian.PutUint32(b[128+24+240+36:], pe.IMAGE_SCN_MEM_READ) }} {
		t.Run(name, func(t *testing.T) {
			bad := append([]byte{}, valid...)
			mutate(bad)
			if VerifyPE(bad) == nil {
				t.Fatal("invalid PE accepted")
			}
		})
	}
	if VerifyPE(valid[:0x200]) == nil || VerifyPE([]byte("not a PE")) == nil {
		t.Fatal("truncated/non-PE accepted")
	}
}
func TestArchivePathsRejectTraversalDevicesAndUserFiles(t *testing.T) {
	for _, name := range []string{"PackageStudio/PackageStudio.exe", "PackageStudio/VERSION", "PackageStudio/README.md", "PackageStudio/THIRD-PARTY-NOTICES.txt", "PackageStudio/adapters/altium/worker.dll", "PackageStudio/licenses/MIT.txt", "PackageStudio/", "PackageStudio/adapters/"} {
		if _, e := archiveRelative(name); e != nil {
			t.Errorf("valid product path rejected %q: %v", name, e)
		}
	}
	for _, name := range []string{"../outside.exe", "/PackageStudio/PackageStudio.exe", "Other/PackageStudio.exe", "PackageStudio/../outside", "PackageStudio/adapters/../../outside", "PackageStudio/adapters\\evil", "PackageStudio/C:/evil.exe", "PackageStudio/licenses/file:stream", "PackageStudio/licenses/NUL.txt", "PackageStudio/licenses/COM1", "PackageStudio/licenses/LPT².log", "PackageStudio/licenses/CON .txt", "PackageStudio/licenses/CONOUT$", "PackageStudio/licenses/space ", "PackageStudio/licenses/dot.", "PackageStudio/licenses/a|b", "PackageStudio/licenses/a?b", "PackageStudio/licenses/a\x00b", "PackageStudio/settings.json", "PackageStudio/parts.json", "PackageStudio/user-document.txt", "PackageStudio/updates/anything", "PackageStudio/licenses/" + strings.Repeat("x", 256)} {
		if _, e := archiveRelative(name); e == nil {
			t.Errorf("unsafe update path accepted %q", name)
		}
	}
}
func TestArchiveVerifiesAllEntriesAndRejectsAliases(t *testing.T) {
	base := normalEntries(t)
	file, m := savedArchive(t, zipBytes(t, base))
	if e := VerifyArchive(file, m); e != nil {
		t.Fatal(e)
	}
	cases := map[string][]zipTestEntry{
		"duplicate-file":      {{name: "PackageStudio/adapters/altium/TEST.dll", data: []byte("alias")}},
		"file-directory":      {{name: "PackageStudio/adapters/altium/test.dll/", mode: os.ModeDir | 0755}},
		"parent-is-file":      {{name: "PackageStudio/licenses/LICENSE", data: []byte("file")}, {name: "PackageStudio/licenses/LICENSE/sub.txt", data: []byte("descendant")}},
		"child-before-file":   {{name: "PackageStudio/licenses/LICENSE/sub.txt", data: []byte("descendant")}, {name: "PackageStudio/licenses/LICENSE", data: []byte("file")}},
		"symlink":             {{name: "PackageStudio/licenses/link", data: []byte("../../elsewhere"), mode: os.ModeSymlink | 0777}},
		"symlink-directory":   {{name: "PackageStudio/licenses/link/", mode: os.ModeSymlink | os.ModeDir | 0777}},
		"device":              {{name: "PackageStudio/licenses/device", mode: os.ModeDevice | 0600}},
		"exe-directory":       {{name: "PackageStudio/PackageStudio.exe/", mode: os.ModeDir | 0755}},
		"user-file":           {{name: "PackageStudio/settings.json", data: []byte("{}")}},
		"duplicate-directory": {{name: "PackageStudio/licenses/", mode: os.ModeDir | 0755}, {name: "PackageStudio/licenses/", mode: os.ModeDir | 0755}},
	}
	for name, extra := range cases {
		t.Run(name, func(t *testing.T) {
			entries := append(append([]zipTestEntry{}, base...), extra...)
			file, m := savedArchive(t, zipBytes(t, entries))
			if VerifyArchive(file, m) == nil {
				t.Fatal("invalid archive accepted")
			}
		})
	}
	t.Run("corrupt-adapter-crc", func(t *testing.T) {
		raw := zipBytes(t, base)
		offset := bytes.Index(raw, []byte("ADAPTER-CHECKSUM-UNIQUE-PAYLOAD"))
		if offset < 0 {
			t.Fatal("stored fixture payload not found")
		}
		raw[offset] ^= 1
		file, m := savedArchive(t, raw)
		if VerifyArchive(file, m) == nil {
			t.Fatal("adapter CRC corruption accepted despite matching archive SHA-256")
		}
	})
	t.Run("missing-main", func(t *testing.T) {
		file, m := savedArchive(t, zipBytes(t, base[1:]))
		if VerifyArchive(file, m) == nil {
			t.Fatal("missing main executable accepted")
		}
	})
	t.Run("wrong-inner-version", func(t *testing.T) {
		entries := append([]zipTestEntry{}, base...)
		entries[1].data = []byte("0.2.0")
		file, m := savedArchive(t, zipBytes(t, entries))
		if VerifyArchive(file, m) == nil {
			t.Fatal("wrong package VERSION accepted")
		}
	})
}

func TestArchiveRejectsWrongHashSizeAndExpansionLimits(t *testing.T) {
	file, m := savedArchive(t, zipBytes(t, normalEntries(t)))
	bad := m
	bad.SHA256 = strings.Repeat("0", 64)
	if VerifyArchive(file, bad) == nil {
		t.Fatal("wrong SHA-256 accepted")
	}
	bad = m
	bad.Size++
	if VerifyArchive(file, bad) == nil {
		t.Fatal("wrong archive size accepted")
	}
	entries := normalEntries(t)
	for i := 0; i < 601; i++ {
		entries = append(entries, zipTestEntry{name: fmt.Sprintf("PackageStudio/licenses/item-%d.txt", i), data: []byte("x")})
	}
	large, m := savedArchive(t, zipBytes(t, entries))
	if VerifyArchive(large, m) == nil {
		t.Fatal("excessive file count accepted")
	}
	z, e := zip.OpenReader(file)
	if e != nil {
		t.Fatal(e)
	}
	defer z.Close()
	z.File[len(z.File)-1].UncompressedSize64 = 129 * 1024 * 1024
	if inspectArchive(z, m.Version) == nil {
		t.Fatal("oversized declared entry accepted")
	}
}

type testTransport func(*http.Request) (*http.Response, error)

func (f testTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }
func response(status int, b []byte) *http.Response {
	return &http.Response{StatusCode: status, Body: io.NopCloser(bytes.NewReader(b)), Header: make(http.Header)}
}
func TestDownloadChecksHashCachesAndNeverLeavesPartialArchive(t *testing.T) {
	raw := zipBytes(t, normalEntries(t))
	_, m := savedArchive(t, raw)
	downloads := 0
	c := NewClient("0.2.0", t.TempDir())
	c.HTTP = &http.Client{Transport: testTransport(func(r *http.Request) (*http.Response, error) {
		switch r.URL.String() {
		case FeedURL:
			return response(200, manifestJSON(t, m)), nil
		case m.DownloadURL:
			downloads++
			return response(200, raw), nil
		default:
			t.Fatalf("unapproved network destination %s", r.URL)
			return nil, errors.New("unexpected request")
		}
	})}
	for i := 0; i < 2; i++ {
		download, e := c.Download(context.Background())
		if e != nil || !download.Verified || download.Version != "0.3.0" {
			t.Fatalf("download failed %#v %v", download, e)
		}
		if e := VerifyArchive(download.Path, m); e != nil {
			t.Fatal(e)
		}
	}
	if downloads != 1 {
		t.Fatal("verified cache was needlessly downloaded again")
	}
	bad := NewClient("0.2.0", t.TempDir())
	bad.HTTP = &http.Client{Transport: testTransport(func(r *http.Request) (*http.Response, error) {
		if r.URL.String() == FeedURL {
			return response(200, manifestJSON(t, m)), nil
		}
		return response(200, raw[:len(raw)-1]), nil
	})}
	if _, e := bad.Download(context.Background()); e == nil {
		t.Fatal("short download accepted")
	}
	entries, e := os.ReadDir(filepath.Join(bad.CacheDir, m.Version))
	if e != nil {
		t.Fatal(e)
	}
	if len(entries) != 0 {
		t.Fatal("failed download left published/temporary files")
	}
}
func TestCheck404AndInvalidManifestNeverOfferDownload(t *testing.T) {
	c := NewClient("0.2.0", t.TempDir())
	c.HTTP = &http.Client{Transport: testTransport(func(r *http.Request) (*http.Response, error) { return response(404, nil), nil })}
	info, e := c.Check(context.Background())
	if e != nil || info.Available || info.Message == "" {
		t.Fatalf("unpublished channel mishandled: %#v %v", info, e)
	}
	m := testManifest("0.3.0")
	m.DownloadURL = "https://attacker.invalid/payload"
	count := 0
	c.HTTP = &http.Client{Transport: testTransport(func(r *http.Request) (*http.Response, error) {
		count++
		if r.URL.String() != FeedURL {
			t.Fatal("invalid feed caused download")
		}
		return response(200, manifestJSON(t, m)), nil
	})}
	if _, e := c.Download(context.Background()); e == nil || count != 1 {
		t.Fatal("invalid manifest was not rejected before download")
	}
}

func writeFixture(t *testing.T, root, rel, content string) {
	t.Helper()
	path := filepath.Join(root, filepath.FromSlash(rel))
	if e := os.MkdirAll(filepath.Dir(path), 0755); e != nil {
		t.Fatal(e)
	}
	if e := os.WriteFile(path, []byte(content), 0644); e != nil {
		t.Fatal(e)
	}
}
func contents(t *testing.T, path string) string {
	t.Helper()
	b, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	return string(b)
}
func TestPublishReplacesProductFilesAndRollbackPreservesUserFiles(t *testing.T) {
	root, stage := t.TempDir(), t.TempDir()
	writeFixture(t, root, "PackageStudio.exe", "old-exe")
	writeFixture(t, root, "VERSION", "old-version")
	writeFixture(t, root, "settings.json", "user-settings")
	writeFixture(t, root, "documents/user.txt", "user-document")
	writeFixture(t, stage, "PackageStudio.exe", "new-exe")
	writeFixture(t, stage, "VERSION", "new-version")
	writeFixture(t, stage, "licenses/new.txt", "license")
	files := []string{"PackageStudio.exe", "VERSION", filepath.FromSlash("licenses/new.txt")}
	tx, e := publish(stage, root, files)
	if e != nil {
		t.Fatal(e)
	}
	if contents(t, filepath.Join(root, "PackageStudio.exe")) != "new-exe" || contents(t, filepath.Join(root, "VERSION")) != "new-version" {
		t.Fatal("new product files were not published")
	}
	if contents(t, filepath.Join(tx.backup, "PackageStudio.exe")) != "old-exe" {
		t.Fatal("old executable backup missing")
	}
	if contents(t, filepath.Join(root, "settings.json")) != "user-settings" || contents(t, filepath.Join(root, "documents/user.txt")) != "user-document" {
		t.Fatal("unrelated user files changed")
	}
	if e := tx.rollback(); e != nil {
		t.Fatal(e)
	}
	if contents(t, filepath.Join(root, "PackageStudio.exe")) != "old-exe" || contents(t, filepath.Join(root, "VERSION")) != "old-version" {
		t.Fatal("rollback did not restore previous version")
	}
	if _, e := os.Stat(filepath.Join(root, "licenses", "new.txt")); !os.IsNotExist(e) {
		t.Fatal("rollback did not remove newly added product file")
	}
}
func TestPublishRejectsUnsafeAndDuplicateFileListBeforeMutation(t *testing.T) {
	for _, files := range [][]string{{"../outside"}, {"settings.json"}, {"VERSION", "VERSION"}, {"VERSION", "version"}} {
		root, stage := t.TempDir(), t.TempDir()
		writeFixture(t, root, "VERSION", "old")
		writeFixture(t, root, "settings.json", "user")
		writeFixture(t, stage, "VERSION", "new")
		writeFixture(t, stage, "settings.json", "replace-user")
		if _, e := publish(stage, root, files); e == nil {
			t.Fatalf("unsafe file list accepted %v", files)
		}
		if contents(t, filepath.Join(root, "VERSION")) != "old" || contents(t, filepath.Join(root, "settings.json")) != "user" {
			t.Fatal("preflight failure changed installation")
		}
	}
}
func TestPublishAutomaticallyRollsBackAfterMidTransactionFailure(t *testing.T) {
	root, stage := t.TempDir(), t.TempDir()
	writeFixture(t, root, "PackageStudio.exe", "old-exe")
	writeFixture(t, root, "VERSION", "old-version")
	writeFixture(t, root, "settings.json", "user-settings")
	if e := os.MkdirAll(filepath.Join(root, "adapters", "blocked.dll"), 0755); e != nil {
		t.Fatal(e)
	}
	writeFixture(t, stage, "PackageStudio.exe", "new-exe")
	writeFixture(t, stage, "VERSION", "new-version")
	writeFixture(t, stage, "adapters/blocked.dll", "new-adapter")
	// VERSION sorts before adapters/. The final adapter target is a directory,
	// so a real filesystem failure occurs after VERSION has been replaced.
	if _, e := publish(stage, root, []string{"PackageStudio.exe", "VERSION", filepath.FromSlash("adapters/blocked.dll")}); e == nil {
		t.Fatal("expected replacement failure")
	}
	if contents(t, filepath.Join(root, "VERSION")) != "old-version" || contents(t, filepath.Join(root, "PackageStudio.exe")) != "old-exe" || contents(t, filepath.Join(root, "settings.json")) != "user-settings" {
		t.Fatal("automatic rollback did not restore previous files and preserve settings")
	}
}
func TestPublishDoesNotCreateDirectoriesThroughSymlink(t *testing.T) {
	root, stage, outside := t.TempDir(), t.TempDir(), t.TempDir()
	if e := os.Symlink(outside, filepath.Join(root, "adapters")); e != nil {
		if runtime.GOOS != "windows" {
			t.Skipf("symlink privileges unavailable: %v", e)
		}
		cmd := exec.Command("cmd.exe", "/c", "mklink", "/J", filepath.Join(root, "adapters"), outside)
		hideWindow(cmd)
		if output, junctionError := cmd.CombinedOutput(); junctionError != nil {
			t.Skipf("junction unavailable: %v %s", junctionError, output)
		}
	}
	writeFixture(t, stage, "adapters/new-folder/file.dll", "new")
	if _, e := publish(stage, root, []string{filepath.FromSlash("adapters/new-folder/file.dll")}); e == nil {
		t.Fatal("redirected parent accepted")
	}
	entries, e := os.ReadDir(outside)
	if e != nil || len(entries) != 0 {
		t.Fatal("created directories through redirect before rejecting it")
	}
}
func TestPrepareAndHealthAreBoundToCacheAndNonce(t *testing.T) {
	cache := t.TempDir()
	raw := zipBytes(t, normalEntries(t))
	_, m := savedArchive(t, raw)
	dir := filepath.Join(cache, m.Version)
	if e := os.MkdirAll(dir, 0700); e != nil {
		t.Fatal(e)
	}
	archive := filepath.Join(dir, ArchiveName)
	if e := os.WriteFile(archive, raw, 0600); e != nil {
		t.Fatal(e)
	}
	if e := os.WriteFile(filepath.Join(dir, "manifest.json"), manifestJSON(t, m), 0600); e != nil {
		t.Fatal(e)
	}
	install := t.TempDir()
	exe := filepath.Join(install, "PackageStudio.exe")
	writeFixture(t, install, "PackageStudio.exe", "current-self-owned-test-placeholder")
	c := NewClient("0.2.0", cache)
	helper, planFile, e := c.Prepare(archive, exe, t.TempDir(), 123)
	if e != nil {
		t.Fatal(e)
	}
	if contents(t, helper) != "current-self-owned-test-placeholder" {
		t.Fatal("helper did not copy current executable")
	}
	var plan Plan
	if json.Unmarshal([]byte(contents(t, planFile)), &plan) != nil || !hashPattern.MatchString(plan.Nonce) || plan.ParentPID != 123 {
		t.Fatal("prepare did not bind plan identity")
	}
	outside, _ := savedArchive(t, raw)
	if _, _, e := c.Prepare(outside, exe, t.TempDir(), 123); e == nil {
		t.Fatal("accepted archive outside own cache")
	}
	c.CurrentVersion = m.Version
	if _, _, e := c.Prepare(archive, exe, t.TempDir(), 123); e == nil {
		t.Fatal("accepted same-version update")
	}
	data := t.TempDir()
	nonce := strings.Repeat("a", 64)
	health := filepath.Join(data, "updates", "0.3.0", "startup-"+nonce+".json")
	if e := os.MkdirAll(filepath.Dir(health), 0700); e != nil {
		t.Fatal(e)
	}
	if e := WriteHealth(data, health, nonce, "0.3.0"); e != nil {
		t.Fatal(e)
	}
	var report Health
	if json.Unmarshal([]byte(contents(t, health)), &report) != nil || report.Nonce != nonce || report.Version != "0.3.0" || report.PID != os.Getpid() {
		t.Fatal("health identity mismatch")
	}
	for _, bad := range []struct{ path, nonce string }{{filepath.Join(t.TempDir(), filepath.Base(health)), nonce}, {health, strings.Repeat("z", 64)}, {filepath.Join(filepath.Dir(health), "different.json"), nonce}} {
		if WriteHealth(data, bad.path, bad.nonce, "0.3.0") == nil {
			t.Fatal("invalid health path/nonce accepted")
		}
	}
}
func TestExtractRejectsMissingEmbeddedVersionOnWindows(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("Windows version resource check")
	}
	file, m := savedArchive(t, zipBytes(t, normalEntries(t)))
	if _, e := extractArchive(file, t.TempDir(), m); e == nil {
		t.Fatal("header-only PE without version resource was accepted for installation")
	}
}
