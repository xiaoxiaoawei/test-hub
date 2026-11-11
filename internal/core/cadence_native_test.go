package core

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestCadenceNativeScriptsUsePrivateSessionsAndReopenData(t *testing.T) {
	parts := DemoParts()
	parts[0].Symbol.Pins[0].Name = "A]\nerror injected; {$x}"
	result, err := ExportCadence(parts, t.TempDir(), "Native Library", true, true)
	if err != nil {
		t.Fatal(err)
	}
	plan, err := PrepareCadenceNative(result.Directory, parts, strings.Repeat("a", 64), true, 100)
	if err != nil {
		t.Fatal(err)
	}
	build, err := os.ReadFile(filepath.Join(result.Directory, filepath.FromSlash(plan.CaptureBuild)))
	if err != nil {
		t.Fatal(err)
	}
	text := string(build)
	for _, token := range []string{"[pid]!=$psExpectedPID", " GetLib ", "PackageExists", "NewPackage", "PackageStudioNonce", "before-inventory.tcldata", "file rename -force", "$psNativeReportPath.tmp"} {
		if !strings.Contains(text, token) {
			t.Fatalf("native build missing %s", token)
		}
	}
	if strings.Index(text, "[pid]!=$psExpectedPID") > strings.Index(text, "DboTclHelper_sCreateSession") {
		t.Fatal("private-process guard occurs after Dbo access")
	}
	if strings.Contains(text, "$psSession CreateLib") {
		t.Fatal("append recreates the existing library")
	}
	readback, err := os.ReadFile(filepath.Join(result.Directory, filepath.FromSlash(plan.CaptureReadback)))
	if err != nil {
		t.Fatal(err)
	}
	for _, token := range []string{"DboTclHelper_sCreateSession", " GetLib ", "GetPackage", "GetPinCount", "GetPin ", "GetPinName", "GetPinNumber", "GetEffectivePropStringValue", "Existing package disappeared", "Existing package pin/footprint inventory changed"} {
		if !strings.Contains(string(readback), token) {
			t.Fatalf("readback missing %s", token)
		}
	}
	for _, job := range plan.Allegro {
		for _, name := range []string{"build.il", "native-readback.il", "native-compiled.il"} {
			raw, e := os.ReadFile(filepath.Join(result.Directory, filepath.FromSlash(job.Directory), name))
			if e != nil {
				t.Fatal(e)
			}
			if e = balancedSkill(string(raw)); e != nil {
				t.Fatalf("%s: %v", name, e)
			}
			if !strings.Contains(string(raw), "renameFile(") {
				t.Fatalf("%s does not publish a complete report atomically", name)
			}
		}
	}
	if _, err = os.Stat(filepath.Join(result.Directory, filepath.FromSlash(plan.CaptureLibrary))); !os.IsNotExist(err) {
		t.Fatal("preparing scripts fabricated an OLB")
	}
}

func TestCadenceNativeTclSyntaxWhenInterpreterAvailable(t *testing.T) {
	python, err := exec.LookPath("python")
	if err != nil {
		t.Skip("optional Tcl syntax checker requires Python tkinter")
	}
	if err = exec.Command(python, "-c", "import tkinter").Run(); err != nil {
		t.Skip("Python tkinter unavailable; generated source assertions still run")
	}
	result, err := ExportCadence(DemoParts(), t.TempDir(), "Syntax", true, false)
	if err != nil {
		t.Fatal(err)
	}
	plan, err := PrepareCadenceNative(result.Directory, DemoParts(), strings.Repeat("b", 64), true, 100)
	if err != nil {
		t.Fatal(err)
	}
	driver := "import sys,tkinter; s=open(sys.argv[1],encoding='utf-8').read(); assert tkinter.Tcl().call('info','complete',s)==1, 'Incomplete Tcl source'"
	for _, rel := range []string{plan.CaptureBuild, plan.CaptureReadback} {
		if output, err := exec.Command(python, "-c", driver, filepath.Join(result.Directory, filepath.FromSlash(rel))).CombinedOutput(); err != nil {
			t.Fatalf("%s: %v %s", rel, err, output)
		}
	}
}
