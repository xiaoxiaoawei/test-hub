package core

import (
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func fixture(t *testing.T, name string) Part {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join("testdata", name+".json"))
	if err != nil {
		t.Fatal(err)
	}
	p, err := ParseEasyEDA(raw, name)
	if err != nil {
		t.Fatal(err)
	}
	return p
}
func near(t *testing.T, got, want float64) {
	t.Helper()
	if math.Abs(got-want) > 1e-6 {
		t.Fatalf("got %.9f, want %.9f", got, want)
	}
}

func TestPublicResistorPreservesPhysicalScaleAndConnectivity(t *testing.T) {
	p := fixture(t, "C25804")
	if p.Name != "0603WAF1002T5E" || len(p.Footprint.Pads) != 2 || len(p.Symbol.Pins) != 2 {
		t.Fatalf("unexpected part: %#v", p)
	}
	pads := map[string]Pad{}
	for _, pad := range p.Footprint.Pads {
		pads[pad.Number] = pad
	}
	near(t, pads["1"].X, -.753364)
	near(t, pads["2"].X, .753364)
	near(t, pads["1"].Width, .8064754)
	near(t, pads["1"].Height, .8640064)
	pins := map[string]Pin{}
	for _, pin := range p.Symbol.Pins {
		pins[pin.Number] = pin
	}
	near(t, pins["1"].X, -5.08)
	near(t, pins["1"].Length, 2.54)
	near(t, pins["1"].Rotation, 0)
	near(t, pins["2"].Rotation, 180)
	if len(pinMappingWarnings(p)) != 0 {
		t.Fatal("two-pin resistor lost its pin/pad correspondence")
	}
	if p.Model == nil || p.Model.UUID != "6bd5cd867e9542ebae21caaf5d2d4c4d" {
		t.Fatal("3D model identifier missing")
	}
}

func TestMCURetainsAll48PinsAcrossTwoUnits(t *testing.T) {
	p := fixture(t, "C23922")
	if p.Symbol.Units != 2 || len(p.Symbol.Pins) != 48 || len(p.Footprint.Pads) != 48 {
		t.Fatalf("lost geometry: units %d pins %d pads %d", p.Symbol.Units, len(p.Symbol.Pins), len(p.Footprint.Pads))
	}
	counts := map[int]int{}
	seen := map[string]bool{}
	for _, pin := range p.Symbol.Pins {
		counts[pin.Unit]++
		if seen[pin.Number] {
			t.Errorf("duplicate physical pin %s", pin.Number)
		}
		seen[pin.Number] = true
	}
	if counts[1] != 41 || counts[2] != 7 {
		t.Fatalf("incorrect unit assignment: %v", counts)
	}
	if w := pinMappingWarnings(p); len(w) != 0 {
		t.Fatal(w)
	}
	text := renderSymbol(p, "STM32F030C8T6", "MyParts", true)
	tree := parseSexpr(t, text)
	if tree[0] != "symbol" {
		t.Fatal("not a symbol")
	}
	children := childNodes(tree, "symbol")
	if len(children) != 2 {
		t.Fatalf("expected two KiCad units, got %d", len(children))
	}
	for i, c := range children {
		pins := childNodes(c, "pin")
		if len(pins) != counts[i+1] {
			t.Fatalf("unit %d: serialized %d pins", i+1, len(pins))
		}
	}
	props := childNodes(tree, "property")
	found := false
	for _, prop := range props {
		if prop[1] == "Footprint" && prop[2] == "MyParts:STM32F030C8T6" {
			found = true
		}
	}
	if !found {
		t.Fatal("symbol footprint link missing")
	}
}

func TestPhysicalPinNumberAndSVGOriginOverrideDeprecatedFields(t *testing.T) {
	raw := []byte(`{"head":{"docType":"2","x":0,"y":0,"c_para":{"name":"pin-test"}},"shape":["P~show~2~99~999~999~90~id~0^^999~999^^M 10 20 v -10~#000^^1~0~0~0~OUT~start^^1~0~0~0~A1~start^^1~0~0^^1~M0 0 L1 1"]}`)
	p, err := ParseEasyEDA(raw, "")
	if err != nil {
		t.Fatal(err)
	}
	pin := p.Symbol.Pins[0]
	if pin.Number != "A1" || pin.ElectricalType != "output" || pin.GraphicStyle != "inverted_clock" {
		t.Fatalf("metadata lost: %#v", pin)
	}
	near(t, pin.X, 2.54)
	near(t, pin.Y, 5.08)
	near(t, pin.Rotation, 270)
	tree := parseSexpr(t, renderSymbol(p, "pin_test", "L", false))
	unit := childNodes(tree, "symbol")[0]
	kpin := childNodes(unit, "pin")[0]
	at := childNodes(kpin, "at")[0]
	if at[1] != "2.540000" || at[2] != "-5.080000" || at[3] != "90.000000" {
		t.Fatalf("KiCad coordinate conversion wrong: %v", at)
	}
}

func TestExportCreatesIndependentBatchesAndPortableModelCopy(t *testing.T) {
	output := t.TempDir()
	sentinel := filepath.Join(output, "MyParts.kicad_sym")
	if err := os.WriteFile(sentinel, []byte("existing library"), 0644); err != nil {
		t.Fatal(err)
	}
	p := DemoParts()[0]
	model := filepath.Join(t.TempDir(), "model.step")
	modelData := []byte("ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n")
	if err := os.WriteFile(model, modelData, 0644); err != nil {
		t.Fatal(err)
	}
	p.Model = &Model{Path: model, ScaleX: 1, ScaleY: 1, ScaleZ: 1}
	a, err := ExportKiCad([]Part{p}, output, "MyParts", true, true)
	if err != nil {
		t.Fatal(err)
	}
	b, err := ExportKiCad([]Part{p}, output, "MyParts", true, true)
	if err != nil {
		t.Fatal(err)
	}
	if a.Directory == b.Directory {
		t.Fatal("second export overwrote first batch")
	}
	old, _ := os.ReadFile(sentinel)
	if string(old) != "existing library" {
		t.Fatal("modified user's old library")
	}
	copyPath := filepath.Join(a.Directory, "MyParts.3dshapes", "R_0603_10k.step")
	copyData, err := os.ReadFile(copyPath)
	if err != nil || string(copyData) != string(modelData) {
		t.Fatal("model copy differs")
	}
	fpRaw, err := os.ReadFile(filepath.Join(a.Directory, "MyParts.pretty", "R_0603_10k.kicad_mod"))
	if err != nil {
		t.Fatal(err)
	}
	fp := parseSexpr(t, string(fpRaw))
	if len(childNodes(fp, "pad")) != 2 {
		t.Fatal("export lost demo pad")
	}
	models := childNodes(fp, "model")
	if len(models) != 1 || models[0][1] != filepath.ToSlash(copyPath) {
		t.Fatal("model reference does not identify published model copy")
	}
	reportRaw, _ := os.ReadFile(filepath.Join(a.Directory, "conversion-report.json"))
	var report map[string]any
	if json.Unmarshal(reportRaw, &report) != nil {
		t.Fatal("invalid conversion report")
	}
	entries, _ := os.ReadDir(output)
	for _, entry := range entries {
		if strings.HasPrefix(entry.Name(), ".packagestudio-") {
			t.Fatal("staging directory was left behind")
		}
	}
}

func TestFailedBatchPublishesNothingAndNamesCannotEscapeDirectory(t *testing.T) {
	output := t.TempDir()
	p := DemoParts()[0]
	p.Name = "../CON/恶意\"\nname"
	p.Model = &Model{Path: filepath.Join(output, "missing.step")}
	_, err := ExportKiCad([]Part{p}, output, "../parts", true, true)
	if err == nil {
		t.Fatal("expected missing model error")
	}
	entries, _ := os.ReadDir(output)
	if len(entries) != 0 {
		t.Fatalf("failed export left partial files: %v", entries)
	}
	p.Model = nil
	result, err := ExportKiCad([]Part{p}, output, "../parts", true, true)
	if err != nil {
		t.Fatal(err)
	}
	for _, file := range result.Files {
		rel, err := filepath.Rel(output, file)
		if err != nil || strings.HasPrefix(rel, "..") {
			t.Fatalf("path traversal: %s", file)
		}
	}
	if _, err := ExportKiCad([]Part{p}, "relative", "parts", true, true); err == nil {
		t.Fatal("relative output accepted")
	}
	if SafeName("CON") != "_CON" {
		t.Fatal("Windows reserved file name unhandled")
	}
}

func TestCustomPadGeometryRemainsAtItsSourceCoordinates(t *testing.T) {
	p := DemoParts()[0]
	p.Footprint.Pads = []Pad{{Number: "A1", X: 5, Y: 6, Width: 2, Height: 2, Shape: "custom", Rotation: 90, Type: "smd", Layer: "F.Cu", Points: []Point{{4, 5}, {6, 5}, {5, 7}}}}
	tree := parseSexpr(t, renderFootprint(p, "custom", ""))
	pad := childNodes(tree, "pad")[0]
	position := childNodes(pad, "at")[0]
	if position[3] != "0.000000" {
		t.Fatal("custom points were rotated a second time")
	}
	primitive := childNodes(childNodes(pad, "primitives")[0], "gr_poly")[0]
	xy := childNodes(childNodes(primitive, "pts")[0], "xy")
	if xy[0][1] != "-1.000000" || xy[0][2] != "-1.000000" || xy[2][1] != "0.000000" || xy[2][2] != "1.000000" {
		t.Fatalf("custom local coordinates wrong: %#v", xy)
	}
}

// An independent small s-expression reader checks generated hierarchy and values.
// It does not share any serializer code and understands quoted/escaped strings.
func parseSexpr(t *testing.T, s string) []any {
	t.Helper()
	i := 0
	var parse func() []any
	skip := func() {
		for i < len(s) && (s[i] == ' ' || s[i] == '\n' || s[i] == '\r' || s[i] == '\t') {
			i++
		}
	}
	parse = func() []any {
		skip()
		if i >= len(s) || s[i] != '(' {
			t.Fatalf("expected '(' at %d", i)
		}
		i++
		node := []any{}
		for {
			skip()
			if i >= len(s) {
				t.Fatal("unterminated expression")
			}
			if s[i] == ')' {
				i++
				return node
			}
			if s[i] == '(' {
				node = append(node, parse())
				continue
			}
			if s[i] == '"' {
				i++
				var b strings.Builder
				for {
					if i >= len(s) {
						t.Fatal("unterminated quoted string")
					}
					c := s[i]
					i++
					if c == '"' {
						break
					}
					if c == '\\' {
						if i >= len(s) {
							t.Fatal("truncated escape")
						}
						c = s[i]
						i++
					}
					b.WriteByte(c)
				}
				node = append(node, b.String())
				continue
			}
			start := i
			for i < len(s) && s[i] != '(' && s[i] != ')' && s[i] != ' ' && s[i] != '\n' && s[i] != '\r' && s[i] != '\t' {
				i++
			}
			if i == start {
				t.Fatal("invalid token")
			}
			node = append(node, s[start:i])
		}
	}
	root := parse()
	skip()
	if i != len(s) {
		t.Fatalf("trailing content at %d", i)
	}
	return root
}
func childNodes(root []any, key string) [][]any {
	out := [][]any{}
	for _, entry := range root {
		if c, ok := entry.([]any); ok && len(c) > 0 && c[0] == key {
			out = append(out, c)
		}
	}
	return out
}
