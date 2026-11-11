package eda

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestPADSRealX86HelperRejectsNonPADSWithoutStartingIt(t *testing.T) {
	ps := filepath.Join(os.Getenv("SystemRoot"), "SysWOW64", "WindowsPowerShell", "v1.0", "powershell.exe")
	if _, err := os.Stat(ps); err != nil {
		t.Skip("32-bit Windows PowerShell unavailable")
	}
	helper, err := filepath.Abs(filepath.Join("..", "..", "helpers", "pads", "native.ps1"))
	if err != nil {
		t.Fatal(err)
	}
	stage, err := os.MkdirTemp(t.TempDir(), ".packagestudio-pads-native-")
	if err != nil {
		t.Fatal(err)
	}
	base := filepath.Join(stage, "native", "Library")
	before := padsOriginalSet(t, base)
	request := map[string]any{"protocol": 1, "nonce": strings.Repeat("a", 48), "phase": "verify", "root": stage, "response": filepath.Join(stage, "response.json"),
		"sdk":       map[string]any{"root": os.Getenv("SystemRoot"), "factory": "", "layout": filepath.Join(os.Getenv("SystemRoot"), "System32", "cmd.exe"), "logic": "", "hashes": map[string]string{}},
		"libraries": []padsLibraryJob{{ID: "library-1", Base: base, Existing: true, PartTypeEngine: "layout"}}}
	raw, _ := json.Marshal(request)
	requestPath := filepath.Join(stage, "request.json")
	if err = os.WriteFile(requestPath, raw, 0600); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, ps, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "RemoteSigned", "-File", helper, "-Request", requestPath)
	padsHideConsole(cmd)
	output, err := cmd.CombinedOutput()
	if ctx.Err() != nil {
		t.Fatalf("negative helper test hung: %v %s", ctx.Err(), output)
	}
	if err == nil || !strings.Contains(string(output), "genuine PADS Classic 9.4") {
		t.Fatalf("expected genuine-engine metadata rejection: %v %s", err, output)
	}
	assertPADSOriginals(t, before)
	if _, err = os.Stat(filepath.Join(stage, "response.json")); !os.IsNotExist(err) {
		t.Fatal("non-PADS engine produced a successful report")
	}
	_ = filepath.WalkDir(stage, func(path string, entry os.DirEntry, err error) error {
		if err == nil && !entry.IsDir() && strings.HasSuffix(strings.ToLower(path), ".exe") {
			t.Error("unqualified system program was copied for CAD execution")
		}
		return err
	})
}

func TestPADSBasicGenerationOnX86PowerShellWithNullCategories(t *testing.T) {
	ps := filepath.Join(os.Getenv("SystemRoot"), "SysWOW64", "WindowsPowerShell", "v1.0", "powershell.exe")
	if _, err := os.Stat(ps); err != nil {
		t.Skip("32-bit Windows PowerShell unavailable")
	}
	generator, err := filepath.Abs(filepath.Join("..", "..", "helpers", "pads", "basic.ps1"))
	if err != nil {
		t.Fatal(err)
	}
	// This is a pure script-generation test. No PADS program, COM object, DLL or
	// activation is involved. Null values match Go's unselected JSON categories.
	script := `$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
. '` + strings.ReplaceAll(generator, "'", "''") + `'
$job=[pscustomobject]@{phase='convert';nonce=('a'*48)}
$symbol=[pscustomobject]@{expected=[pscustomobject]@{parts=@('PART');decals=$null;symbols=@('PART_U1');drawings=$null};pcb='';schematic='symbol.c';partTypes='symbol.p';partTypeEngine='logic'}
$body=New-StudioPADSBasic $job $symbol 'logic' $true 'private.exe' 'inventory.txt' 'library'
if($body.Contains('GetLibraryItems(1,') -or $body.Contains('GetLibraryItems(3,') -or $body.Contains('GetLibraryItems(0, "")')) {throw 'Null symbol-only categories produced an empty-name query.'}
if(-not $body.Contains('GetLibraryItems(2, "PART_U1")') -or -not $body.Contains('ImportLibraryItems2("symbol.c", 2)') -or -not $body.Contains('ImportLibraryItems2("symbol.p", 2)')) {throw 'Symbol-only expected queries/imports missing.'}
if($body.IndexOf('Private INI guard') -gt $body.IndexOf('Set lib =')) {throw 'Library access preceded private configuration guard.'}
if(-not $body.Contains('"PID|" & CStr(GetCurrentProcessId())') -or -not $body.Contains('Application.Visible = True')) {throw 'Owned process identity or normal window closing is not available.'}
$footprint=[pscustomobject]@{expected=[pscustomobject]@{parts=@('PART');decals=@('PART');symbols=$null;drawings=$null};pcb='footprint.d';schematic='';partTypes='footprint.p';partTypeEngine='layout'}
$body=New-StudioPADSBasic $job $footprint 'layout' $true 'private.exe' 'inventory.txt' 'library'
if($body.Contains('GetLibraryItems(2,') -or $body.Contains('GetLibraryItems(3,') -or $body.Contains('GetLibraryItems(1, "")')) {throw 'Null footprint-only categories produced an empty-name query.'}
if(-not $body.Contains('GetLibraryItems(1, "PART")') -or -not $body.Contains('ImportLibraryItems2("footprint.d", 2)') -or -not $body.Contains('ImportLibraryItems2("footprint.p", 2)') -or -not $body.Contains('counted <> collection.Count')) {throw 'Footprint-only imports or SDK count check missing.'}
$job.phase='verify'
$body=New-StudioPADSBasic $job $footprint 'layout' $true 'private.exe' 'inventory.txt' 'library'
if($body.Contains('ImportLibraryItems2')) {throw 'Readback script writes a library.'}
Write-Output 'Pure PADS Basic generator passed symbol-only, footprint-only and readback cases.'
`
	path := filepath.Join(t.TempDir(), "generate.ps1")
	if err = os.WriteFile(path, append([]byte{0xef, 0xbb, 0xbf}, []byte(script)...), 0600); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, ps, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "RemoteSigned", "-File", path)
	padsHideConsole(cmd)
	if output, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("Basic generator failed on its real x86 host: %v %s", err, output)
	}
}
