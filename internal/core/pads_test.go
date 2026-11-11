package core

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func padsFile(t *testing.T, result ExportResult, extension string) string {
	t.Helper()
	for _, path := range result.Files {
		if filepath.Ext(path) == extension {
			raw, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if !strings.Contains(string(raw), "\r\n") {
				t.Fatal("PADS records require CRLF line endings")
			}
			for _, v := range raw {
				if v > 127 {
					t.Fatal("non-ASCII bytes entered the V9 interchange file")
				}
			}
			return string(raw)
		}
	}
	t.Fatalf("no %s file in export", extension)
	return ""
}

func TestPADSMetricDecalAndImperialLogicPreserveResistorMapping(t *testing.T) {
	p := DemoParts()[0]
	result, err := ExportPADS([]Part{p}, t.TempDir(), "MyParts", true, true)
	if err != nil {
		t.Fatal(err)
	}
	pcb := readPADSPCBTest(t, padsFile(t, result, ".d"))
	sch := readPADSSCHTest(t, padsFile(t, result, ".c"))
	types := readPADSTypesTest(t, padsFile(t, result, ".p"))
	if len(pcb) != 1 || len(sch) != 1 || len(types) != 1 {
		t.Fatal("unexpected PADS record counts")
	}
	d, c, part := pcb[0], sch[0], types[0]
	if d.units != "M" || len(d.terminals) != 2 || len(d.stacks) != 2 {
		t.Fatalf("invalid decal units/counts: %#v", d)
	}
	near(t, d.terminals[0].x, -.825)
	near(t, d.terminals[0].y, 0)
	for _, stack := range d.stacks {
		if len(stack.layers) != 5 {
			t.Fatal("SMD stack lost mask or paste layer")
		}
		top := stack.layers[-2]
		if top[2] != "RF" {
			t.Fatalf("round rectangle lost: %v", top)
		}
		near(t, padsTestFloat(t, top[1]), .8)
		near(t, padsTestFloat(t, top[3]), .12)
		near(t, padsTestFloat(t, top[4]), 90)
		near(t, padsTestFloat(t, top[5]), .95)
		if padsTestFloat(t, stack.layers[-1][1]) != 0 || padsTestFloat(t, stack.layers[0][1]) != 0 {
			t.Fatal("SMD pad gained inner/bottom copper")
		}
	}
	if part.decal != d.name || part.gates[0].decal != c.name || len(c.terminals) != 2 || len(part.gates[0].pins) != 2 {
		t.Fatal("PCB/CAE/Part Type references differ")
	}
	if part.gates[0].pins[0].number != "1" || part.gates[0].pins[1].number != "2" {
		t.Fatal("physical pins changed")
	}
	// The source resistor's body ends at ±100 mil. Standard PADS PIN is 200 mil.
	// Outer endpoints therefore become ±300 mil while the body remains unchanged.
	near(t, c.terminals[0].x, -300)
	near(t, c.terminals[1].x, 300)
	if c.terminals[0].mirror != 0 || c.terminals[1].mirror != 2 {
		t.Fatal("Logic inward pin direction changed")
	}
	if len(result.Warnings) == 0 {
		t.Fatal("standard PIN adaptation must be disclosed")
	}
}

func TestPADS48PinMCUHasTwoCompleteGates(t *testing.T) {
	p := fixture(t, "C23922")
	result, err := ExportPADS([]Part{p}, t.TempDir(), "MCUParts", true, true)
	if err != nil {
		t.Fatal(err)
	}
	pcb := readPADSPCBTest(t, padsFile(t, result, ".d"))[0]
	sch := readPADSSCHTest(t, padsFile(t, result, ".c"))
	part := readPADSTypesTest(t, padsFile(t, result, ".p"))[0]
	if len(pcb.terminals) != 48 || len(pcb.stacks) != 48 || len(part.gates) != 2 || len(sch) != 2 {
		t.Fatalf("lost MCU connectivity: terminals %d stacks %d gates %d decals %d", len(pcb.terminals), len(pcb.stacks), len(part.gates), len(sch))
	}
	if len(part.gates[0].pins) != 41 || len(part.gates[1].pins) != 7 {
		t.Fatal("MCU power unit was flattened or lost")
	}
	seen := map[string]bool{}
	names := map[string]bool{}
	for i, gate := range part.gates {
		if gate.decal != sch[i].name || len(gate.pins) != len(sch[i].terminals) {
			t.Fatal("gate terminal order/count cannot be resolved")
		}
		for _, pin := range gate.pins {
			if seen[pin.number] {
				t.Fatal("physical pin duplicated across gates")
			}
			seen[pin.number] = true
			if names[pin.name] {
				t.Fatal("PADS pin names must be unique")
			}
			names[pin.name] = true
		}
	}
	for n := 1; n <= 48; n++ {
		if !seen[strconv.Itoa(n)] {
			t.Fatalf("MCU pin %d disappeared", n)
		}
	}
}

func TestPADSAlphanumericPinMapUsesDecalTerminalOrder(t *testing.T) {
	p := DemoParts()[0]
	p.Name = "BGA Map Test"
	p.Footprint.Pads = []Pad{}
	p.Symbol.Pins = []Pin{}
	numbers := []string{"A2", "B1", "A1"}
	for i, number := range numbers {
		p.Footprint.Pads = append(p.Footprint.Pads, Pad{Number: number, X: float64(i + 1), Y: 2, Width: 1, Height: 1, Shape: "circle", Type: "smd", Layer: "F.Cu"})
	}
	for i, number := range []string{"A1", "A2", "B1"} {
		p.Symbol.Pins = append(p.Symbol.Pins, Pin{Number: number, Name: "IO", X: -5, Y: float64(i * 3), Length: 5.08, Rotation: 0, ElectricalType: "bidirectional", Unit: 1})
	}
	result, err := ExportPADS([]Part{p}, t.TempDir(), "Map", true, true)
	if err != nil {
		t.Fatal(err)
	}
	pcb := readPADSPCBTest(t, padsFile(t, result, ".d"))[0]
	part := readPADSTypesTest(t, padsFile(t, result, ".p"))[0]
	if strings.Join(part.pinmap, ",") != "A2,B1,A1" {
		t.Fatalf("pin map is not in PCB terminal order: %v", part.pinmap)
	}
	physicalToX := map[string]float64{}
	for i, terminal := range pcb.terminals {
		if terminal.number != strconv.Itoa(i+1) {
			t.Fatal("PCB terminal IDs must be sequential numeric indices")
		}
		physicalToX[part.pinmap[i]] = terminal.x
	}
	near(t, physicalToX["A1"], 3)
	near(t, physicalToX["A2"], 1)
	near(t, physicalToX["B1"], 2)
	if part.gates[0].pins[0].number != "A1" {
		t.Fatal("gate list was sorted independently of schematic terminals")
	}
}

func TestPADSCardinalPinEndpointsRetainBodyAttachment(t *testing.T) {
	p := DemoParts()[0]
	p.Symbol.Pins = []Pin{{Number: "1", X: -7.62, Y: 0, Length: 2.54, Rotation: 0, Unit: 1}, {Number: "2", X: 0, Y: -7.62, Length: 2.54, Rotation: 90, Unit: 1}, {Number: "3", X: 7.62, Y: 0, Length: 2.54, Rotation: 180, Unit: 1}, {Number: "4", X: 0, Y: 7.62, Length: 2.54, Rotation: 270, Unit: 1}}
	result, err := ExportPADS([]Part{p}, t.TempDir(), "Directions", true, false)
	if err != nil {
		t.Fatal(err)
	}
	sch := readPADSSCHTest(t, padsFile(t, result, ".c"))[0]
	want := []struct {
		x, y, dx, dy float64
		rtn, mirror  int
	}{{-200, 0, 1, 0, 0, 0}, {0, 200, 0, -1, 90, 2}, {200, 0, -1, 0, 0, 2}, {0, -200, 0, 1, 90, 0}}
	for i, terminal := range sch.terminals {
		near(t, terminal.x+want[i].dx*200, want[i].x)
		near(t, terminal.y+want[i].dy*200, want[i].y)
		if terminal.rtn != want[i].rtn || terminal.mirror != want[i].mirror {
			t.Fatalf("terminal %d direction is invalid: %#v", i, terminal)
		}
	}
}

func TestPADSSlotAndBottomSMDHaveCorrectPadStackRoles(t *testing.T) {
	p := DemoParts()[0]
	p.Footprint.Pads = []Pad{{Number: "1", X: 2, Y: 3, Width: 2, Height: 1.2, Shape: "oval", Rotation: 30, Drill: .6, DrillWidth: 1.2, Type: "thru_hole", Layer: "*.Cu"}, {Number: "2", X: -2, Y: -3, Width: 1.5, Height: .7, Shape: "rect", Rotation: 45, Type: "smd", Layer: "B.Cu"}}
	result, err := ExportPADS([]Part{p}, t.TempDir(), "Stack", false, true)
	if err != nil {
		t.Fatal(err)
	}
	d := readPADSPCBTest(t, padsFile(t, result, ".d"))[0]
	near(t, d.terminals[0].x, 2)
	near(t, d.terminals[0].y, -3)
	slot := d.stacks[1]
	if slot.plated != "P" || len(slot.slot) != 3 {
		t.Fatal("slotted plated drill metadata lost")
	}
	near(t, slot.drill, .6)
	near(t, padsTestFloat(t, slot.slot[0]), 150)
	near(t, padsTestFloat(t, slot.slot[1]), 1.2)
	for _, layer := range []int{-2, -1, 0} {
		if slot.layers[layer][2] != "OF" {
			t.Fatal("through-hole copper lost")
		}
	}
	bottom := d.stacks[2]
	if bottom.layers[0][2] != "RF" || padsTestFloat(t, bottom.layers[-2][1]) != 0 || padsTestFloat(t, bottom.layers[-1][1]) != 0 {
		t.Fatal("bottom SMD was placed on top/inner copper")
	}
	if _, ok := bottom.layers[28]; !ok {
		t.Fatal("bottom mask missing")
	}
	if _, ok := bottom.layers[22]; !ok {
		t.Fatal("bottom paste missing")
	}
}

func TestPADSBatchIsIndependentAndFailureDoesNotPublish(t *testing.T) {
	dir := t.TempDir()
	old := filepath.Join(dir, "old.pt9")
	if err := os.WriteFile(old, []byte("existing"), 0644); err != nil {
		t.Fatal(err)
	}
	p := DemoParts()[0]
	a, err := ExportPADS([]Part{p}, dir, "../保留 PADS", true, true)
	if err != nil {
		t.Fatal(err)
	}
	b, err := ExportPADS([]Part{p}, dir, "../保留 PADS", true, true)
	if err != nil {
		t.Fatal(err)
	}
	if a.Directory == b.Directory {
		t.Fatal("previous batch overwritten")
	}
	oldBytes, _ := os.ReadFile(old)
	if string(oldBytes) != "existing" {
		t.Fatal("old native library changed")
	}
	before, _ := os.ReadDir(dir)
	p.Footprint.Pads[0].Shape = "custom"
	p.Footprint.Pads[0].Points = []Point{{0, 0}, {1, 0}, {0, 1}}
	if _, err := ExportPADS([]Part{p}, dir, "Unsupported", true, true); err == nil {
		t.Fatal("custom copper was silently approximated")
	}
	after, _ := os.ReadDir(dir)
	if len(before) != len(after) {
		t.Fatal("failed batch left files behind")
	}
	for _, file := range a.Files {
		rel, err := filepath.Rel(dir, file)
		if err != nil || strings.HasPrefix(rel, "..") {
			t.Fatal("file escaped output directory")
		}
		if ext := filepath.Ext(file); ext == ".pt9" || ext == ".pd9" || ext == ".ld9" || ext == ".ln9" {
			t.Fatal("writer falsely emitted a proprietary native library")
		}
	}
}

func TestPADSStepIsAnAttachmentAndOptionsSelectRealFileTypes(t *testing.T) {
	p := DemoParts()[0]
	model := filepath.Join(t.TempDir(), "source.step")
	modelBytes := []byte("ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n")
	if err := os.WriteFile(model, modelBytes, 0644); err != nil {
		t.Fatal(err)
	}
	p.Model = &Model{Path: model}
	result, err := ExportPADS([]Part{p}, t.TempDir(), "Models", false, true)
	if err != nil {
		t.Fatal(err)
	}
	d := padsFile(t, result, ".d")
	if strings.Contains(d, "*REMARK* AP") {
		t.Fatal("writer claimed automatic 3D association")
	}
	copied := false
	for _, file := range result.Files {
		if filepath.Ext(file) == ".step" {
			raw, _ := os.ReadFile(file)
			copied = string(raw) == string(modelBytes)
		}
		if filepath.Ext(file) == ".c" {
			t.Fatal("unexpected schematic decal in footprint-only export")
		}
	}
	if !copied {
		t.Fatal("STEP attachment missing")
	}
	var report struct {
		NativeLibraryGenerated bool              `json:"nativeLibraryGenerated"`
		Parts                  []json.RawMessage `json:"parts"`
	}
	reportBytes, _ := os.ReadFile(filepath.Join(result.Directory, "conversion-report.json"))
	if json.Unmarshal(reportBytes, &report) != nil || report.NativeLibraryGenerated || len(report.Parts) != 1 {
		t.Fatal("report misrepresents native library creation")
	}
	p.Model = nil
	symbolOnly, err := ExportPADS([]Part{p}, t.TempDir(), "Symbols", true, false)
	if err != nil {
		t.Fatal(err)
	}
	part := readPADSTypesTest(t, padsFile(t, symbolOnly, ".p"))[0]
	if part.decal != "0" || len(part.pinmap) != 0 {
		t.Fatal("symbol-only library referenced a nonexistent PCB decal")
	}
	for _, file := range symbolOnly.Files {
		if filepath.Ext(file) == ".d" {
			t.Fatal("unexpected PCB file in symbol-only export")
		}
	}
}

// The following reader follows the published V9 record counts independently.
// It is not used by the serializer, and checks cross-file positional mappings.
type padsTestCursor struct {
	t     *testing.T
	lines []string
	index int
}

func newPADSCursor(t *testing.T, raw, header string) *padsTestCursor {
	t.Helper()
	lines := []string{}
	for _, line := range strings.Split(strings.ReplaceAll(raw, "\r\n", "\n"), "\n") {
		if strings.TrimSpace(line) != "" {
			lines = append(lines, strings.TrimSpace(line))
		}
	}
	c := &padsTestCursor{t: t, lines: lines}
	if c.next() != header {
		t.Fatal("wrong PADS library header")
	}
	return c
}
func (c *padsTestCursor) next() string {
	c.t.Helper()
	if c.index >= len(c.lines) {
		c.t.Fatal("unexpected end of PADS file")
	}
	line := c.lines[c.index]
	c.index++
	return line
}
func (c *padsTestCursor) end() bool {
	c.t.Helper()
	if c.index < len(c.lines) && c.lines[c.index] == "*END*" {
		c.index++
		if c.index != len(c.lines) {
			c.t.Fatal("records after END")
		}
		return true
	}
	return false
}
func (c *padsTestCursor) timestamp() {
	c.t.Helper()
	if !strings.HasPrefix(c.next(), "TIMESTAMP ") {
		c.t.Fatal("missing timestamp")
	}
}
func padsTestInt(t *testing.T, s string) int {
	t.Helper()
	n, err := strconv.Atoi(s)
	if err != nil {
		t.Fatalf("invalid integer %q", s)
	}
	return n
}
func padsTestFloat(t *testing.T, s string) float64 {
	t.Helper()
	n, err := strconv.ParseFloat(s, 64)
	if err != nil || math.IsNaN(n) || math.IsInf(n, 0) {
		t.Fatalf("invalid coordinate %q", s)
	}
	return n
}
func (c *padsTestCursor) pieces(count int) {
	c.t.Helper()
	for i := 0; i < count; i++ {
		h := strings.Fields(c.next())
		if len(h) != 5 {
			c.t.Fatalf("bad piece header: %v", h)
		}
		switch h[0] {
		case "OPEN", "CLOSED", "CIRCLE", "COPCLS":
		default:
			c.t.Fatalf("unrecognized piece kind %s", h[0])
		}
		n := padsTestInt(c.t, h[1])
		for j := 0; j < n; j++ {
			point := strings.Fields(c.next())
			if len(point) != 2 {
				c.t.Fatalf("bad point record: %v", point)
			}
			padsTestFloat(c.t, point[0])
			padsTestFloat(c.t, point[1])
		}
	}
}

type padsTestTerminal struct {
	x, y        float64
	number      string
	rtn, mirror int
}
type padsTestStack struct {
	plated string
	drill  float64
	slot   []string
	layers map[int][]string
}
type padsTestPCB struct {
	name, units string
	terminals   []padsTestTerminal
	stacks      map[int]padsTestStack
}

func readPADSPCBTest(t *testing.T, raw string) []padsTestPCB {
	t.Helper()
	c := newPADSCursor(t, raw, "*PADS-LIBRARY-PCB-DECALS-V9*")
	out := []padsTestPCB{}
	for !c.end() {
		h := strings.Fields(c.next())
		if len(h) != 11 {
			t.Fatalf("bad PCB header %v", h)
		}
		d := padsTestPCB{name: h[0], units: h[1], stacks: map[int]padsTestStack{}}
		c.timestamp()
		for i := 0; i < padsTestInt(t, h[4]); i++ {
			c.next()
		}
		for i := 0; i < padsTestInt(t, h[5]); i++ {
			c.next()
			c.next()
		}
		c.pieces(padsTestInt(t, h[6]))
		for i := 0; i < padsTestInt(t, h[7]); i++ {
			c.next()
			c.next()
		}
		for i := 0; i < padsTestInt(t, h[8]); i++ {
			line := strings.Fields(c.next())
			if len(line) != 5 || !strings.HasPrefix(line[0], "T") {
				t.Fatalf("invalid PCB terminal: %v", line)
			}
			d.terminals = append(d.terminals, padsTestTerminal{x: padsTestFloat(t, line[0][1:]), y: padsTestFloat(t, line[1]), number: line[4]})
		}
		for i := 0; i < padsTestInt(t, h[9]); i++ {
			line := strings.Fields(c.next())
			if len(line) != 5 && len(line) != 8 {
				t.Fatalf("invalid PAD header: %v", line)
			}
			if line[0] != "PAD" {
				t.Fatal("missing PAD stack")
			}
			stack := padsTestStack{plated: line[3], drill: padsTestFloat(t, line[4]), layers: map[int][]string{}}
			if len(line) == 8 {
				stack.slot = line[5:]
			}
			for j := 0; j < padsTestInt(t, line[2]); j++ {
				layer := strings.Fields(c.next())
				if len(layer) < 3 {
					t.Fatal("invalid pad stack layer")
				}
				switch layer[2] {
				case "R":
					if len(layer) != 3 {
						t.Fatal("invalid round pad record")
					}
				case "RF":
					if len(layer) != 7 {
						t.Fatal("invalid rectangular finger record")
					}
				case "OF":
					if len(layer) != 6 {
						t.Fatal("invalid oval finger record")
					}
				default:
					t.Fatalf("unsupported shape in test reader %s", layer[2])
				}
				stack.layers[padsTestInt(t, layer[0])] = layer
			}
			d.stacks[padsTestInt(t, line[1])] = stack
		}
		out = append(out, d)
	}
	return out
}

type padsTestSCH struct {
	name      string
	terminals []padsTestTerminal
}

func readPADSSCHTest(t *testing.T, raw string) []padsTestSCH {
	t.Helper()
	c := newPADSCursor(t, raw, "*PADS-LIBRARY-SCH-DECALS-V9*")
	out := []padsTestSCH{}
	for !c.end() {
		h := strings.Fields(c.next())
		if len(h) != 12 {
			t.Fatalf("bad SCH header: %v", h)
		}
		d := padsTestSCH{name: h[0]}
		c.timestamp()
		for i := 0; i < 2; i++ {
			if c.next() != "\"Default Font\"" {
				t.Fatal("missing schematic font")
			}
		}
		for i := 0; i < padsTestInt(t, h[7]); i++ {
			c.next()
			c.next()
		}
		c.pieces(padsTestInt(t, h[8]))
		for i := 0; i < padsTestInt(t, h[9]); i++ {
			c.next()
			c.next()
		}
		for i := 0; i < padsTestInt(t, h[10]); i++ {
			line := strings.Fields(c.next())
			if len(line) != 13 || !strings.HasPrefix(line[0], "T") || line[12] != "PIN" {
				t.Fatalf("invalid SCH terminal %v", line)
			}
			terminal := padsTestTerminal{x: padsTestFloat(t, line[0][1:]), y: padsTestFloat(t, line[1]), rtn: padsTestInt(t, line[2]), mirror: padsTestInt(t, line[3])}
			if terminal.rtn != 0 && terminal.rtn != 90 {
				t.Fatal("invalid terminal rotation")
			}
			d.terminals = append(d.terminals, terminal)
			props := strings.Fields(c.next())
			if len(props) != 9 || !strings.HasPrefix(props[0], "P") {
				t.Fatal("missing pin-label location line")
			}
		}
		out = append(out, d)
	}
	return out
}

type padsTestPin struct{ number, typ, name string }
type padsTestGate struct {
	decal string
	pins  []padsTestPin
}
type padsTestType struct {
	name, decal string
	gates       []padsTestGate
	pinmap      []string
}

func readPADSTypesTest(t *testing.T, raw string) []padsTestType {
	t.Helper()
	c := newPADSCursor(t, raw, "*PADS-LIBRARY-PART-TYPES-V9*")
	out := []padsTestType{}
	for !c.end() {
		h := strings.Fields(c.next())
		if len(h) != 9 {
			t.Fatalf("bad Part Type header %v", h)
		}
		part := padsTestType{name: h[0], decal: h[1]}
		c.timestamp()
		for i := 0; i < padsTestInt(t, h[4]); i++ {
			if !strings.HasPrefix(c.next(), "\"") {
				t.Fatal("bad attribute")
			}
		}
		for i := 0; i < padsTestInt(t, h[5]); i++ {
			gateHeader := strings.Fields(c.next())
			if len(gateHeader) != 4 || gateHeader[0] != "GATE" {
				t.Fatalf("bad gate header %v", gateHeader)
			}
			gate := padsTestGate{}
			for j := 0; j < padsTestInt(t, gateHeader[1]); j++ {
				gate.decal = c.next()
			}
			for j := 0; j < padsTestInt(t, gateHeader[2]); j++ {
				pin := strings.Fields(c.next())
				if len(pin) != 4 {
					t.Fatalf("bad gate pin %v", pin)
				}
				if !strings.Contains("SB COTLZPGU", pin[2]) {
					t.Fatal("bad electrical pin type")
				}
				gate.pins = append(gate.pins, padsTestPin{number: pin[0], typ: pin[2], name: pin[3]})
			}
			part.gates = append(part.gates, gate)
		}
		for i := 0; i < padsTestInt(t, h[6]); i++ {
			if !strings.HasPrefix(c.next(), "SIGPIN ") {
				t.Fatal("bad SIGPIN")
			}
		}
		count := padsTestInt(t, h[7])
		for len(part.pinmap) < count {
			part.pinmap = append(part.pinmap, strings.Fields(c.next())...)
		}
		if len(part.pinmap) != count {
			t.Fatal("wrong pin-map count")
		}
		out = append(out, part)
	}
	return out
}

func ExampleExportPADS() {
	fmt.Println("PADS .d/.c/.p are ASCII interchange files; import them into a new PADS library before use.")
	// Output: PADS .d/.c/.p are ASCII interchange files; import them into a new PADS library before use.
}
