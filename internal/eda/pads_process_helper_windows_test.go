package eda

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestPADSOwnedProcessCompletionRequiresActualZeroExit(t *testing.T) {
	ps := filepath.Join(os.Getenv("SystemRoot"), "SysWOW64", "WindowsPowerShell", "v1.0", "powershell.exe")
	if _, e := os.Stat(ps); e != nil {
		t.Skip("x86 PowerShell unavailable")
	}
	helper, e := filepath.Abs(filepath.Join("..", "..", "helpers", "pads", "process.ps1"))
	if e != nil {
		t.Fatal(e)
	}
	script := `$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
. '` + strings.ReplaceAll(helper, "'", "''") + `'
Confirm-StudioPADSPID '123' 123
foreach($reported in @('','0','not-a-pid','124')) {
 $failed=$false
 try{Confirm-StudioPADSPID $reported 123}catch{$failed=$true}
 if(-not $failed){throw 'Mismatched or invalid owner PID was accepted.'}
}
function Fixture([bool]$Exited,[bool]$Close,[bool]$Wait,[int]$Code) {
 $f=[pscustomobject]@{HasExited=$Exited;CloseResult=$Close;WaitResult=$Wait;ExitCode=$Code;CloseCalls=0;WaitCalls=0}
 $f|Add-Member ScriptMethod CloseMainWindow {$this.CloseCalls++;return $this.CloseResult}
 $f|Add-Member ScriptMethod WaitForExit {param($timeout);$this.WaitCalls++;if($this.WaitResult){$this.HasExited=$true};return $this.WaitResult}
 return $f
}
function MustFail($f,[string]$Expected) {
 $errorText=''
 try {Complete-StudioPADSProcess $f 100} catch {$errorText=$_.Exception.Message}
 if(-not $errorText.Contains($Expected)) {throw ('Expected rejected process '+$Expected+', got '+$errorText)}
}
$f=Fixture $true $false $false 0
Complete-StudioPADSProcess $f 100
if($f.CloseCalls -ne 0 -or $f.WaitCalls -ne 0){throw 'Already exited process was closed again.'}
$f=Fixture $false $true $true 0
Complete-StudioPADSProcess $f 100
if($f.CloseCalls -ne 1 -or $f.WaitCalls -ne 1 -or -not $f.HasExited){throw 'Normal owned window close was not confirmed.'}
MustFail (Fixture $true $false $false 19) 'exited with code 19'
MustFail (Fixture $false $false $true 0) 'no closable main window'
MustFail (Fixture $false $true $false 0) 'did not exit cleanly'
MustFail (Fixture $false $true $true 23) 'exited with code 23'
# Also exercise real, headless owned Windows processes. These are system test
# children, not PADS, and no GUI/application/document is opened.
foreach($code in @(0,17)) {
 $start=New-Object Diagnostics.ProcessStartInfo
 $start.FileName=(Join-Path $env:SystemRoot 'SysWOW64\WindowsPowerShell\v1.0\powershell.exe')
 $start.Arguments=('-NoProfile -NonInteractive -Command "exit '+$code+'"')
 $start.UseShellExecute=$false;$start.CreateNoWindow=$true
 $p=[Diagnostics.Process]::Start($start)
 try {
  if(-not $p.WaitForExit(10000)){throw 'Owned test child did not finish.'}
  if($code -eq 0){Complete-StudioPADSProcess $p 100}else{MustFail $p 'exited with code 17'}
 } finally {if(-not $p.HasExited){$p.Kill();$p.WaitForExit()};$p.Dispose()}
}
Write-Output 'Owned process completion checks passed.'
`
	path := filepath.Join(t.TempDir(), "process-test.ps1")
	if e = os.WriteFile(path, append([]byte{0xef, 0xbb, 0xbf}, []byte(script)...), 0600); e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, ps, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "RemoteSigned", "-File", path)
	padsHideConsole(cmd)
	if out, e := cmd.CombinedOutput(); e != nil {
		t.Fatalf("owned-process helper test failed: %v %s", e, out)
	}
	for _, name := range []string{"models.ps1", "native.ps1"} {
		data, e := os.ReadFile(filepath.Join(filepath.Dir(helper), name))
		if e != nil {
			t.Fatal(e)
		}
		body := string(data)
		check := strings.Index(body, "Complete-StudioPADSProcess $process")
		success := "return $inventory"
		if name == "models.ps1" {
			success = "$result.items+="
		}
		if check < 0 || check > strings.Index(body, success) {
			t.Fatalf("%s accepts evidence before clean process exit", name)
		}
		if !strings.Contains(body, "cleanup did not finish") {
			t.Fatalf("%s does not bound owned-process cleanup", name)
		}
		if !strings.Contains(body, "Confirm-StudioPADSPID") || !strings.Contains(body, "$process.Id") {
			t.Fatalf("%s does not bind evidence to the spawned PID", name)
		}
	}
}
