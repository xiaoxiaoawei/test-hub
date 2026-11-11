package core

import (
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestCadenceAtomicScriptPackage(t *testing.T) {
	output := t.TempDir()
	parts := DemoParts()
	result, err := ExportCadence(parts, output, "Example ../库", false, true)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(result.Directory, output+string(os.PathSeparator)) {
		t.Fatal("batch escaped destination")
	}
	data, err := os.ReadFile(filepath.Join(result.Directory, "cadence-manifest.json"))
	if err != nil {
		t.Fatal(err)
	}
	var m CadenceManifest
	if err = json.Unmarshal(data, &m); err != nil {
		t.Fatal(err)
	}
	if m.Status != "scripts-only" || len(m.Allegro) != 1 || len(m.ExpectedNativeFiles) != 3 {
		t.Fatalf("wrong manifest: %#v", m)
	}
	for _, native := range m.ExpectedNativeFiles {
		if _, err = os.Stat(filepath.Join(result.Directory, filepath.FromSlash(native))); !os.IsNotExist(err) {
			t.Fatalf("native file must not be fabricated: %s", native)
		}
		for _, file := range result.Files {
			if filepath.ToSlash(file) == filepath.ToSlash(filepath.Join(result.Directory, native)) {
				t.Fatalf("expected native file claimed as existing: %s", file)
			}
		}
	}
	skill, err := os.ReadFile(filepath.Join(result.Directory, filepath.FromSlash(m.Allegro[0].Directory), "build.il"))
	if err != nil {
		t.Fatal(err)
	}
	if err = balancedSkill(string(skill)); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(skill), `axlDBChangeDesignUnits("millimeters" 4)`) {
		t.Fatal("millimetre conversion absent")
	}
	if !strings.Contains(string(skill), `list(-0.825000 0.000000) pintext 0.000000`) {
		t.Fatal("left pad coordinate was changed")
	}
	if !strings.Contains(string(skill), `list(-0.400000 0.500000)`) {
		t.Fatal("screen Y was not inverted for silk")
	}
	if strings.Count(string(skill), "axlDBCreatePadStack(") != 1 || strings.Count(string(skill), "pin = axlDBCreatePin(") != 2 {
		t.Fatal("identical padstack was not shared by the two pads")
	}
	for _, p := range result.Files {
		if _, err = os.Stat(p); err != nil {
			t.Fatalf("claimed file absent: %v", err)
		}
	}
	second, err := ExportCadence(parts, output, "Example ../库", false, true)
	if err != nil {
		t.Fatal(err)
	}
	if second.Directory == result.Directory {
		t.Fatal("second batch overwrote the first")
	}
	if _, err = os.Stat(result.Directory); err != nil {
		t.Fatal("first batch removed")
	}
}

func TestCadenceGeometryFailurePublishesNothing(t *testing.T) {
	for _, name := range []string{"custom", "slot", "nan", "unknown-layer"} {
		t.Run(name, func(t *testing.T) {
			output := t.TempDir()
			parts := DemoParts()
			switch name {
			case "custom":
				parts[0].Footprint.Pads[0].Shape = "custom"
			case "slot":
				parts[0].Footprint.Pads[0].Drill = .3
				parts[0].Footprint.Pads[0].DrillWidth = .8
			case "nan":
				parts[0].Footprint.Lines[0].X1 = math.NaN()
			case "unknown-layer":
				parts[0].Footprint.Lines[0].Layer = "unknown"
			}
			if _, err := ExportCadence(parts, output, "Reject", false, true); err == nil {
				t.Fatal("invalid/unsupported geometry accepted")
			}
			entries, err := os.ReadDir(output)
			if err != nil {
				t.Fatal(err)
			}
			if len(entries) != 0 {
				t.Fatalf("partial batch left behind: %v", entries)
			}
		})
	}
}

func TestCadenceScriptEscapingAndPinRotation(t *testing.T) {
	p := DemoParts()[0]
	p.Footprint.Pads[0].Rotation = 90
	p.Footprint.Texts = []Text{{Text: "A\")\nerror(\"injected\")", X: 1, Y: 2, Size: 1, Layer: "F.SilkS"}}
	skill, _, _, err := renderCadenceAllegro(p, "Safe_Entry")
	if err != nil {
		t.Fatal(err)
	}
	if err = balancedSkill(skill); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(skill, `list(-0.825000 0.000000) pintext -90.000000`) {
		t.Fatal("pad angle did not convert with Y inversion")
	}
	if strings.Contains(skill, "\nerror(\"injected\")") {
		t.Fatal("text escaped its string literal")
	}
	if !strings.Contains(skill, `A\")\nerror(\"injected\")`) {
		t.Fatal("text was not retained as escaped data")
	}
}

func TestCadenceDiscoveryIsNotRuntimeValidation(t *testing.T) {
	root := t.TempDir()
	tool := filepath.Join(root, "tools", "bin", "allegro.exe")
	if err := os.MkdirAll(filepath.Dir(tool), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(tool, []byte("not an executable; discovery fixture"), 0755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("CDSROOT", root)
	found := false
	for _, candidate := range DetectCadenceTools() {
		if candidate.Executable == tool {
			found = true
			if candidate.RuntimeVerified {
				t.Fatal("file existence was misreported as runtime validation")
			}
		}
	}
	if !found {
		t.Fatal("explicit installation root was not discovered")
	}
}

func TestCadenceCaptureScriptIsNotAnOLB(t *testing.T) {
	parts := DemoParts()
	parts[0].Symbol.Pins[0].Name = "A]\nerror injected; {$x}"
	result, err := ExportCadence(parts, t.TempDir(), "CaptureOnly", true, false)
	if err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(filepath.Join(result.Directory, "cadence-manifest.json"))
	if err != nil {
		t.Fatal(err)
	}
	var m CadenceManifest
	if err = json.Unmarshal(data, &m); err != nil {
		t.Fatal(err)
	}
	if m.CaptureScript == "" || m.CaptureLibrary == "" || len(m.Allegro) != 0 {
		t.Fatal("Capture job missing from manifest")
	}
	if _, err = os.Stat(filepath.Join(result.Directory, filepath.FromSlash(m.CaptureLibrary))); !os.IsNotExist(err) {
		t.Fatal("OLB must only be generated by Capture")
	}
	script, err := os.ReadFile(filepath.Join(result.Directory, filepath.FromSlash(m.CaptureScript)))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(script), `A\]\nerror injected; \{\$x\}`) {
		t.Fatal("pin name escaped Tcl literal")
	}
	for _, token := range []string{"DboTclHelper_sCreateSession", " NewPackage ", " NewCell ", " NewPart ", " NewDevice ", "NewSymbolPinScalar", "NewPinNumber", "SavePackageAll", "SaveLib", "capture-complete.txt"} {
		if !strings.Contains(string(script), token) {
			t.Fatalf("Capture workflow missing %s", token)
		}
	}
	if !strings.Contains(string(script), "UNVERIFIED compatibility setting") {
		t.Fatal("Capture scale qualification missing")
	}
}

func TestCapturePinEndpointsAndUnits(t *testing.T) {
	part := DemoParts()[0]
	part.Symbol.Units = 2
	part.Symbol.Pins[0].Unit = 1
	part.Symbol.Pins[0].X = 0
	part.Symbol.Pins[0].Y = -5.08
	part.Symbol.Pins[0].Rotation = 90
	part.Symbol.Pins[1].Unit = 2
	script, _, err := renderCadenceCapture([]Part{part}, []string{"Resistor"}, "Lib")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(script, "set psStart [psPoint 0.000000 -2.540000]\n set psHot [psPoint 0.000000 -5.080000]") {
		t.Fatal("pin inward start/hot endpoint relationship is wrong")
	}
	if strings.Count(script, " NewDevice ") != 2 || strings.Count(script, " NewSymbolPinScalar ") != 2 {
		t.Fatal("multiunit devices or pins were dropped")
	}
	if !strings.Contains(script, `"B"] 1 $psCell`) {
		t.Fatal("second gate device position is not preserved")
	}
}

func balancedSkill(s string) error {
	depth := 0
	quote, escape, comment := false, false, false
	for _, r := range s {
		if comment {
			if r == '\n' {
				comment = false
			}
			continue
		}
		if quote {
			if escape {
				escape = false
			} else if r == '\\' {
				escape = true
			} else if r == '"' {
				quote = false
			}
			continue
		}
		switch r {
		case ';':
			comment = true
		case '"':
			quote = true
		case '(':
			depth++
		case ')':
			depth--
			if depth < 0 {
				return &cadenceSyntaxError{"unexpected closing parenthesis"}
			}
		}
	}
	if quote || depth != 0 {
		return &cadenceSyntaxError{"unbalanced generated SKILL"}
	}
	return nil
}

type cadenceSyntaxError struct{ message string }

func (e *cadenceSyntaxError) Error() string { return e.message }
