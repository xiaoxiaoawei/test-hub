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

func TestPADSModelBasicGenerationOnRealX86Host(t *testing.T) {
	ps := filepath.Join(os.Getenv("SystemRoot"), "SysWOW64", "WindowsPowerShell", "v1.0", "powershell.exe")
	if _, e := os.Stat(ps); e != nil {
		t.Skip("x86 PowerShell unavailable")
	}
	generator, e := filepath.Abs(filepath.Join("..", "..", "helpers", "pads", "models_basic.ps1"))
	if e != nil {
		t.Fatal(e)
	}
	script := `$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
. '` + strings.ReplaceAll(generator, "'", "''") + `'
$job=[pscustomobject]@{phase='convert';nonce=('a'*48);reuse='private-reuse';pcb='fixture.d';partTypes='fixture.p'}
$item=[pscustomobject]@{entry='PART';decal='PART';existing=$false;step='prepared.step'}
$body=New-StudioPADSModelBasic $job $item 'private.exe' 'private-default.pcb' 'owned.pcb' 'private-lib' 'netlist.asc' 'record.txt'
foreach($guard in @('Private INI guard','Unexpected process executable','Private Reuse guard')) {
 if($body.IndexOf($guard) -lt 0 -or $body.IndexOf($guard) -gt $body.IndexOf('Application.CreateLibrary') -or $body.IndexOf($guard) -gt $body.IndexOf('Application.OpenDocument')) {throw 'A CAD write/open occurs before the private guard.'}
}
if($body.IndexOf('System3D bridge belongs to a different process') -gt $body.IndexOf('Set sys =') -or -not $body.Contains('GetWindowThreadProcessId bridge.Gui.HWND')) {throw 'Native bridge ownership guard missing.'}
if(-not $body.Contains('maps.ImportModel "prepared.step", "PART", "PART"') -or -not $body.Contains('maps.ExportModels "private-reuse"') -or -not $body.Contains('BoundingBox(2, nativeComponent.UniqueId)')) {throw 'Actual SDK calls missing.'}
if($body.Contains('XDAlignment') -or $body.Contains('GetToken') -or $body.Contains('CreateObject')) {throw 'Unexpected private ABI or global activation.'}
if(-not $body.Contains('"PID|" & CStr(GetCurrentProcessId())') -or $body.LastIndexOf('doc.Save') -lt $body.IndexOf('Set box =') -or $body.LastIndexOf('doc.Save') -gt $body.IndexOf('DONE|')) {throw 'Model evidence lacks its actual PID or saved private fixture.'}
$job.phase='verify'
$body=New-StudioPADSModelBasic $job $item 'private.exe' 'private-default.pcb' 'owned.pcb' 'private-lib' 'netlist.asc' 'record.txt'
if($body.Contains('maps.ImportModel') -or $body.Contains('maps.ExportModels')) {throw 'Fresh readback mutates mappings.'}
$job.phase='convert';$item.existing=$true
$body=New-StudioPADSModelBasic $job $item 'private.exe' 'private-default.pcb' 'owned.pcb' 'private-lib' 'netlist.asc' 'record.txt'
if($body.Contains('maps.ImportModel') -or $body.Contains('maps.ExportModels')) {throw 'Append replaces an old mapping.'}
Write-Output 'Model Basic generator guards, imports, readback and old-map preservation passed.'
`
	path := filepath.Join(t.TempDir(), "generate-models.ps1")
	if e = os.WriteFile(path, append([]byte{0xef, 0xbb, 0xbf}, []byte(script)...), 0600); e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, ps, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "RemoteSigned", "-File", path)
	padsHideConsole(cmd)
	if output, e := cmd.CombinedOutput(); e != nil {
		t.Fatalf("pure generator failed on real host: %v %s", e, output)
	}
}

func TestPADSModelHelperRejectsSystemExecutableBeforeCopyOrStart(t *testing.T) {
	ps := filepath.Join(os.Getenv("SystemRoot"), "SysWOW64", "WindowsPowerShell", "v1.0", "powershell.exe")
	if _, e := os.Stat(ps); e != nil {
		t.Skip("x86 PowerShell unavailable")
	}
	helper, e := filepath.Abs(filepath.Join("..", "..", "helpers", "pads", "models.ps1"))
	if e != nil {
		t.Fatal(e)
	}
	root, e := os.MkdirTemp(t.TempDir(), ".packagestudio-pads-models-")
	if e != nil {
		t.Fatal(e)
	}
	request := map[string]any{"protocol": 1, "nonce": strings.Repeat("a", 48), "phase": "convert", "root": root, "response": filepath.Join(root, "response.json"), "reuse": filepath.Join(root, "reuse"), "pcb": filepath.Join(root, "fixture.d"), "partTypes": filepath.Join(root, "fixture.p"), "sdk": map[string]any{"executable": filepath.Join(os.Getenv("SystemRoot"), "System32", "cmd.exe"), "root": os.Getenv("SystemRoot"), "hashes": map[string]string{}}, "items": []padsModelItem{}}
	raw, _ := json.Marshal(request)
	path := filepath.Join(root, "request.json")
	if e = os.WriteFile(path, raw, 0600); e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, ps, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "RemoteSigned", "-File", helper, "-Request", path)
	padsHideConsole(cmd)
	output, e := cmd.CombinedOutput()
	if e == nil || !strings.Contains(string(output), "candidate PADS Classic 2019.2") {
		t.Fatalf("expected metadata rejection: %v %s", e, output)
	}
	if _, e = os.Stat(filepath.Join(root, "response.json")); !os.IsNotExist(e) {
		t.Fatal("wrong executable generated success")
	}
	filepath.WalkDir(root, func(path string, d os.DirEntry, e error) error {
		if e == nil && strings.EqualFold(filepath.Ext(path), ".exe") {
			t.Error("unqualified EXE copied for startup")
		}
		return e
	})
}
