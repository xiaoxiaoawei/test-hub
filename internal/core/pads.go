package core

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// PADS library ASCII V9 is documented in Mentor Graphics' PADS 9.4
// Parts Library ASCII File Format Specification, pp. 12–27.
// This writer produces interchange files, not the proprietary .pt9/.pd9/etc.
const padsFormatSource = "https://www.freecalypso.org/pub/CAD/PADS/pdfdocs/Plib_ASCII.pdf"

var padsIdentifier = regexp.MustCompile(`[^A-Z0-9_.$-]+`)
var padsPinIdentifier = regexp.MustCompile(`^[A-Za-z0-9_.$+-]{1,7}$`)

type padsPiece struct {
	Kind   string
	Points []Point
	Width  float64
	Layer  int
}
type padsTerminal struct {
	Index      int    `json:"pcbTerminal"`
	Physical   string `json:"physicalPin"`
	Pad        Pad    `json:"pad"`
	Mechanical bool   `json:"mechanical,omitempty"`
}
type padsGate struct {
	Name string `json:"decal"`
	Unit int    `json:"unit"`
	Pins []Pin  `json:"pins"`
}
type padsPlan struct {
	Part      Part
	Name      string
	Terminals []padsTerminal
	Gates     []padsGate
	Warnings  []string
}

// ExportPADS publishes a fresh V9 ASCII library batch. PCB decals use mm;
// schematic decals always use mils as required by the PADS library format.
func ExportPADS(parts []Part, outputDirectory, libraryName string, includeSymbol, includeFootprint bool) (ExportResult, error) {
	result := ExportResult{Files: []string{}, Warnings: []string{}}
	if len(parts) == 0 {
		return result, errors.New("请先选择需要导出的器件")
	}
	if !includeSymbol && !includeFootprint {
		return result, errors.New("请至少选择原理图或 PCB 封装")
	}
	if !filepath.IsAbs(outputDirectory) {
		return result, errors.New("请选择完整的导出目录路径")
	}
	plans := make([]padsPlan, 0, len(parts))
	used := map[string]bool{}
	for _, part := range parts {
		if err := ValidatePart(part, includeSymbol, includeFootprint); err != nil {
			return result, fmt.Errorf("%s：%w", part.Name, err)
		}
		name := padsName(SafeName(part.Name))
		base := name
		for i := 2; used[name]; i++ {
			name = padsName(fmt.Sprintf("%s_%d", base, i))
		}
		used[name] = true
		plan, err := newPADSPlan(part, name, includeSymbol, includeFootprint)
		if err != nil {
			return result, fmt.Errorf("%s：%w", part.Name, err)
		}
		plans = append(plans, plan)
	}
	if err := os.MkdirAll(outputDirectory, 0755); err != nil {
		return result, err
	}
	stage, err := os.MkdirTemp(outputDirectory, ".packagestudio-pads-")
	if err != nil {
		return result, err
	}
	defer os.RemoveAll(stage)
	fileBase := padsName(SafeName(libraryName))
	destination := filepath.Join(outputDirectory, fileBase+"_PADS_"+time.Now().Format("20060102_150405")+"_"+strings.TrimPrefix(filepath.Base(stage), ".packagestudio-pads-"))
	write := func(rel string, data []byte) error {
		path := filepath.Join(stage, rel)
		if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
			return err
		}
		if err := os.WriteFile(path, data, 0644); err != nil {
			return err
		}
		result.Files = append(result.Files, filepath.Join(destination, rel))
		return nil
	}
	stamp := time.Now().UTC().Format("2006.01.02.15.04.05")
	var pcb, sch, types strings.Builder
	pcb.WriteString("*PADS-LIBRARY-PCB-DECALS-V9*\n\n")
	sch.WriteString("*PADS-LIBRARY-SCH-DECALS-V9*\n\n")
	types.WriteString("*PADS-LIBRARY-PART-TYPES-V9*\n\n")
	for i := range plans {
		plan := &plans[i]
		if includeFootprint {
			record, warnings, err := renderPADSPCB(*plan, stamp)
			if err != nil {
				return result, fmt.Errorf("%s：%w", plan.Part.Name, err)
			}
			pcb.WriteString(record)
			plan.Warnings = append(plan.Warnings, warnings...)
		}
		if includeSymbol {
			sch.WriteString(renderPADSSchematic(*plan, stamp))
		}
		types.WriteString(renderPADSPartType(*plan, stamp, includeSymbol, includeFootprint))
		if includeFootprint && plan.Part.Model != nil && plan.Part.Model.Path != "" {
			ext := strings.ToLower(filepath.Ext(plan.Part.Model.Path))
			if ext != ".step" && ext != ".stp" {
				return result, errors.New("PADS 模型附件只接受 STEP 文件")
			}
			src, err := os.Open(plan.Part.Model.Path)
			if err != nil {
				return result, fmt.Errorf("无法读取 STEP 附件：%w", err)
			}
			info, statErr := src.Stat()
			if statErr != nil || !info.Mode().IsRegular() || info.Size() > 128*1024*1024 {
				src.Close()
				return result, errors.New("STEP 附件必须是不超过 128 MiB 的普通文件")
			}
			rel := filepath.Join("models", plan.Name+ext)
			if err := os.MkdirAll(filepath.Dir(filepath.Join(stage, rel)), 0755); err != nil {
				src.Close()
				return result, err
			}
			dst, err := os.Create(filepath.Join(stage, rel))
			if err != nil {
				src.Close()
				return result, err
			}
			_, copyErr := io.Copy(dst, src)
			closeErr := dst.Close()
			src.Close()
			if copyErr != nil {
				return result, copyErr
			}
			if closeErr != nil {
				return result, closeErr
			}
			result.Files = append(result.Files, filepath.Join(destination, rel))
			plan.Warnings = append(plan.Warnings, "STEP 作为独立附件保存，未自动建立 PADS Reuse/3DModels 关联；请在 Layout 中手动添加并定位。")
		}
		plan.Warnings = unique(plan.Warnings)
		for _, warning := range plan.Warnings {
			result.Warnings = append(result.Warnings, plan.Part.Name+"："+warning)
		}
	}
	pcb.WriteString("*END*\n")
	sch.WriteString("*END*\n")
	types.WriteString("*END*\n")
	if includeFootprint {
		if err := write(fileBase+".d", padsCRLF(pcb.String())); err != nil {
			return result, err
		}
	}
	if includeSymbol {
		if err := write(fileBase+".c", padsCRLF(sch.String())); err != nil {
			return result, err
		}
	}
	if err := write(fileBase+".p", padsCRLF(types.String())); err != nil {
		return result, err
	}
	type reportPart struct {
		ID          string         `json:"id"`
		Name        string         `json:"name"`
		Entry       string         `json:"entry"`
		Source      string         `json:"source"`
		SourceURL   string         `json:"sourceUrl"`
		Attribution string         `json:"attribution"`
		Terminals   []padsTerminal `json:"terminals"`
		Gates       []padsGate     `json:"gates"`
		Warnings    []string       `json:"warnings"`
	}
	records := []reportPart{}
	for _, plan := range plans {
		records = append(records, reportPart{plan.Part.ID, plan.Part.Name, plan.Name, plan.Part.Source, plan.Part.SourceURL, plan.Part.Attribution, plan.Terminals, plan.Gates, plan.Warnings})
	}
	report, _ := json.MarshalIndent(map[string]any{"generator": "Package Studio", "format": "PADS Parts Library ASCII V9", "specification": padsFormatSource, "nativeLibraryGenerated": false, "pcbUnits": "mm", "schematicUnits": "mil", "parts": records, "warnings": unique(result.Warnings)}, "", "  ")
	if err := write("conversion-report.json", report); err != nil {
		return result, err
	}
	layerJSON, _ := json.MarshalIndent(map[string]any{"profile": "common PADS standard-layer numbering; verify target document associations before CAM", "layers": map[string]int{"F.Cu": 1, "B.Cu": 2, "F.CrtYd": 20, "B.CrtYd": 20, "F.Mask": 21, "B.Paste": 22, "F.Paste": 23, "Edge.Cuts_reference_only": 24, "Dwgs.User": 25, "Cmts.User": 25, "F.SilkS": 26, "F.Fab": 27, "B.Mask": 28, "B.SilkS": 29, "B.Fab": 30}, "padStackCodes": map[string]int{"top": -2, "inner": -1, "bottom": 0}}, "", "  ")
	if err := write("layer-mapping.json", layerJSON); err != nil {
		return result, err
	}
	if err := write("导入说明.txt", []byte(padsReadme(fileBase, includeSymbol, includeFootprint, result.Warnings))); err != nil {
		return result, err
	}
	if err := os.Rename(stage, destination); err != nil {
		return result, fmt.Errorf("无法发布 PADS 导出批次：%w", err)
	}
	result.Directory = destination
	result.Warnings = unique(result.Warnings)
	return result, nil
}

func padsName(value string) string {
	value = padsIdentifier.ReplaceAllString(strings.ToUpper(value), "_")
	value = strings.Trim(value, "_.")
	if value == "" {
		value = "COMPONENT"
	}
	if len(value) > 40 {
		h := sha256.Sum256([]byte(value))
		value = value[:31] + "_" + strings.ToUpper(hex.EncodeToString(h[:4]))
	}
	return value
}
func padsASCII(value string, max int) string {
	var b strings.Builder
	for _, r := range value {
		if r == '\r' || r == '\n' || r == '\t' {
			b.WriteByte(' ')
		} else if r >= 32 && r <= 126 {
			b.WriteRune(r)
		} else {
			b.WriteByte('_')
		}
		if b.Len() >= max {
			break
		}
	}
	return strings.TrimSpace(b.String())
}
func padsN(v float64) string {
	if math.Abs(v) < .000005 {
		v = 0
	}
	return strconv.FormatFloat(v, 'f', 5, 64)
}
func padsCRLF(s string) []byte   { return []byte(strings.ReplaceAll(s, "\n", "\r\n")) }
func padsMil(mm float64) float64 { return mm / 0.0254 }

func newPADSPlan(p Part, name string, symbol, footprint bool) (padsPlan, error) {
	plan := padsPlan{Part: p, Name: name, Terminals: []padsTerminal{}, Gates: []padsGate{}, Warnings: append([]string{}, p.Warnings...)}
	physical := map[string]bool{}
	if footprint {
		holes := 0
		for _, pad := range p.Footprint.Pads {
			mechanical := pad.Type == "np_thru_hole"
			number := strings.TrimSpace(pad.Number)
			if number == "" && mechanical {
				holes++
				number = fmt.Sprintf("MH%d", holes)
			}
			if !padsPinIdentifier.MatchString(number) {
				return plan, fmt.Errorf("PADS V9 物理引脚号必须为 1–7 位 ASCII 字母/数字，收到 %q", number)
			}
			if physical[number] {
				return plan, fmt.Errorf("PADS 当前尚不支持同号多焊盘 %s，已停止导出以保持电气映射", number)
			}
			physical[number] = true
			if _, err := padsPadShape(pad); err != nil {
				return plan, fmt.Errorf("焊盘 %s：%w", number, err)
			}
			plan.Terminals = append(plan.Terminals, padsTerminal{Index: len(plan.Terminals) + 1, Physical: number, Pad: pad, Mechanical: mechanical})
		}
	}
	if len(plan.Terminals) > 32767 {
		return plan, errors.New("PADS V9 PCB 端子数超过 32767")
	}
	if symbol {
		units := p.Symbol.Units
		if units < 1 {
			units = 1
		}
		if units > 702 {
			return plan, errors.New("PADS gate 数超过 702")
		}
		seenPins := map[string]bool{}
		for u := 1; u <= units; u++ {
			gate := padsGate{Name: padsName(fmt.Sprintf("%s_U%d", name, u)), Unit: u, Pins: []Pin{}}
			for _, pin := range p.Symbol.Pins {
				if unit(pin.Unit) != u {
					continue
				}
				if !padsPinIdentifier.MatchString(pin.Number) {
					return plan, fmt.Errorf("PADS V9 符号引脚号不受支持：%q", pin.Number)
				}
				if seenPins[pin.Number] {
					return plan, fmt.Errorf("同一物理引脚 %s 出现在多个 gate；当前未推断公共引脚连接", pin.Number)
				}
				seenPins[pin.Number] = true
				if footprint && !physical[pin.Number] {
					return plan, fmt.Errorf("符号引脚 %s 没有对应 PCB 焊盘，请核对源数据或单独导出", pin.Number)
				}
				if math.Abs(angle(pin.Rotation)/90-math.Round(angle(pin.Rotation)/90)) > .00001 {
					return plan, fmt.Errorf("PADS Logic 暂不支持非正交引脚 %s", pin.Number)
				}
				gate.Pins = append(gate.Pins, pin)
				if math.Abs(padsMil(pin.Length)-200) > .01 {
					plan.Warnings = append(plan.Warnings, "Logic 使用随 PADS 提供的标准 200 mil PIN 图元；非 200 mil 引脚会保持本体连接点并延伸外部端点，原长度保存在报告中。")
				}
				if pin.GraphicStyle != "" && pin.GraphicStyle != "line" {
					plan.Warnings = append(plan.Warnings, "Logic 引脚使用标准 PIN 图元；反相/时钟图形尚未专门复刻，请按报告核对。")
				}
				if pin.Hidden {
					plan.Warnings = append(plan.Warnings, "源隐藏引脚在本次 Logic 库中显式显示，未创建隐式网络连接。")
				}
			}
			if len(gate.Pins) > 2048 {
				return plan, errors.New("单个 PADS Logic decal 的端子数超过 2048")
			}
			if len(gate.Pins) > 0 {
				plan.Gates = append(plan.Gates, gate)
			}
		}
		if len(plan.Gates) == 0 {
			return plan, errors.New("没有可输出的 Logic gate")
		}
		if footprint {
			for _, t := range plan.Terminals {
				if !t.Mechanical && !seenPins[t.Physical] {
					return plan, fmt.Errorf("PCB 焊盘 %s 没有对应 Logic 引脚，请核对源数据或单独导出", t.Physical)
				}
			}
		}
	}
	if footprint {
		plan.Warnings = append(plan.Warnings, "制造图层采用附带 layer-mapping.json 的标准编号；导入后必须核对目标 Layout 的层类型及上下元件层关联。", "当前统一模型未携带每焊盘的局部阻焊/锡膏扩展值；默认开窗按铜尺寸 1:1 输出，有源显式开窗时沿用源图形。")
	}
	if symbol {
		plan.Warnings = append(plan.Warnings, "PADS gate 的引脚名称会净化并唯一化，物理编号保持不变；被动/未连接等无直接对应的电气类型使用 U，源属性保留在报告。")
	}
	if name != p.Name {
		plan.Warnings = append(plan.Warnings, "PADS 条目名称转换为不超过 40 字符的 ASCII 大写标识；原名称保留在 conversion-report.json。")
	}
	return plan, nil
}

// Shape text follows PAD-layer records in the V9 specification. Finger-pad
// orientation is the long-axis angle in Y-up coordinates, modulo 180 degrees.
func padsPadShape(p Pad) (string, error) {
	if p.Width <= 0 || p.Height <= 0 {
		return "", errors.New("焊盘宽高无效")
	}
	shape := p.Shape
	if shape == "ellipse" {
		shape = "oval"
	}
	if shape == "custom" {
		return "", errors.New("任意多边形焊盘尚未实现无损 PADS 转换，未以外接矩形替换")
	}
	w, h := p.Width, p.Height
	rotation := -p.Rotation
	if w < h {
		w, h = h, w
		rotation += 90
	}
	rotation = math.Mod(rotation+360, 180)
	if shape == "circle" || shape == "oval" {
		if math.Abs(w-h) < .00001 {
			return padsN(w) + " R", nil
		}
		return padsN(h) + " OF " + padsN(rotation) + " " + padsN(w) + " 0", nil
	}
	if shape == "rect" || shape == "roundrect" {
		corner := 0.
		if shape == "roundrect" {
			corner = h * .15
		}
		return padsN(h) + " RF " + padsN(corner) + " " + padsN(rotation) + " " + padsN(w) + " 0", nil
	}
	return "", fmt.Errorf("不支持的 PADS 焊盘形状 %q", shape)
}

func padsLayer(layer string) (int, bool) {
	n, ok := map[string]int{"F.Cu": 1, "B.Cu": 2, "F.CrtYd": 20, "B.CrtYd": 20, "F.Mask": 21, "B.Paste": 22, "F.Paste": 23, "Edge.Cuts": 24, "Dwgs.User": 25, "Cmts.User": 25, "F.SilkS": 26, "F.Fab": 27, "B.Mask": 28, "B.SilkS": 29, "B.Fab": 30}[layer]
	if layer == "" {
		return 26, true
	}
	if !ok {
		return 25, false
	}
	return n, true
}
func padsWritePiece(b *strings.Builder, p padsPiece) {
	fmt.Fprintf(b, "%s %d %s %d -1\n", p.Kind, len(p.Points), padsN(p.Width), p.Layer)
	for _, pt := range p.Points {
		fmt.Fprintf(b, "%s %s\n", padsN(pt.X), padsN(pt.Y))
	}
}
func padsClosed(points []Point) []Point {
	out := append([]Point{}, points...)
	if len(out) > 0 && out[0] != out[len(out)-1] {
		out = append(out, out[0])
	}
	return out
}
func padsYUp(points []Point, scale float64) []Point {
	out := make([]Point, len(points))
	for i, p := range points {
		out[i] = Point{p.X * scale, -p.Y * scale}
	}
	return out
}

func renderPADSPCB(plan padsPlan, stamp string) (string, []string, error) {
	var b strings.Builder
	p := plan.Part
	pieces := []padsPiece{}
	warnings := []string{}
	explicit := map[string]bool{}
	mapLayer := func(layer string) (int, error) {
		n, ok := padsLayer(layer)
		if !ok {
			if strings.Contains(layer, ".Cu") {
				return 0, fmt.Errorf("暂不支持封装内部铜层 %s", layer)
			}
			warnings = append(warnings, "未识别图层 "+layer+" 已作为 Layer 25 文档图形保留。")
		}
		if layer == "Edge.Cuts" {
			warnings = append(warnings, "Edge.Cuts 仅作为 Layer 24 机械参考图形保留，未声明为实际铣槽/板切口。")
		}
		if layer == "F.Paste" || layer == "B.Paste" || layer == "F.Mask" || layer == "B.Mask" {
			explicit[layer] = true
		}
		return n, nil
	}
	for _, l := range p.Footprint.Lines {
		layer, err := mapLayer(l.Layer)
		if err != nil {
			return "", warnings, err
		}
		pieces = append(pieces, padsPiece{Kind: "OPEN", Points: padsYUp([]Point{{l.X1, l.Y1}, {l.X2, l.Y2}}, 1), Width: math.Max(l.Width, .001), Layer: layer})
	}
	for _, c := range p.Footprint.Circles {
		layer, err := mapLayer(c.Layer)
		if err != nil {
			return "", warnings, err
		}
		pieces = append(pieces, padsPiece{Kind: "CIRCLE", Points: padsYUp([]Point{{c.X - c.Radius, c.Y}, {c.X + c.Radius, c.Y}}, 1), Width: math.Max(c.Width, .001), Layer: layer})
	}
	for _, poly := range p.Footprint.Polygons {
		if len(poly.Points) < 3 {
			continue
		}
		layer, err := mapLayer(poly.Layer)
		if err != nil {
			return "", warnings, err
		}
		kind := "CLOSED"
		if poly.Fill {
			kind = "COPCLS"
		}
		pieces = append(pieces, padsPiece{Kind: kind, Points: padsClosed(padsYUp(poly.Points, 1)), Width: math.Max(poly.Width, 0), Layer: layer})
	}
	if len(pieces) > 32767 {
		return "", warnings, errors.New("PADS 当前图形片段数超过安全上限")
	}
	texts := []Text{}
	for _, text := range p.Footprint.Texts {
		if text.Text != "" {
			texts = append(texts, text)
		}
	}
	fmt.Fprintf(&b, "%s M 0 0 0 2 %d %d %d %d 0\nTIMESTAMP %s\n", plan.Name, len(pieces), len(texts), len(plan.Terminals), len(plan.Terminals), stamp)
	fmt.Fprintf(&b, "0 %s 0 0 1.27000 0.12700 26 1 33 \"Default Font\"\nREF-DES\n0 %s 0 0 1.27000 0.12700 27 1 34 \"Default Font\"\nPART-TYPE\n", padsN(-p.Footprint.Bounds.Y+1.27), padsN(-p.Footprint.Bounds.Y-p.Footprint.Bounds.Height-1.27))
	for _, piece := range pieces {
		padsWritePiece(&b, piece)
	}
	for _, t := range texts {
		layer, err := mapLayer(t.Layer)
		if err != nil {
			return "", warnings, err
		}
		text := padsASCII(t.Text, 255)
		if text != t.Text {
			warnings = append(warnings, "PCB 自由文本含非 ASCII 字符或换行，已净化；原文保留在源器件缓存。")
		}
		fmt.Fprintf(&b, "%s %s %s %d %s 0.12700 0 0 0 0 \"Default Font\"\n%s\n", padsN(t.X), padsN(-t.Y), padsN(angle(-t.Rotation)), layer, padsN(math.Max(.254, math.Min(25.4, t.Size))), text)
	}
	for _, t := range plan.Terminals {
		fmt.Fprintf(&b, "T%s %s %s %s %d\n", padsN(t.Pad.X), padsN(-t.Pad.Y), padsN(t.Pad.X), padsN(-t.Pad.Y), t.Index)
	}
	for _, t := range plan.Terminals {
		pad := t.Pad
		shape, err := padsPadShape(pad)
		if err != nil {
			return "", warnings, err
		}
		stack := []string{}
		plated := "N"
		if pad.Drill > 0 && pad.Type != "np_thru_hole" {
			plated = "P"
		}
		if t.Mechanical {
			stack = append(stack, "-2 0 R", "-1 0 R", "0 0 R", "21 "+padsN(pad.Drill)+" R", "28 "+padsN(pad.Drill)+" R")
			warnings = append(warnings, "非金属孔作为独立无铜机械端子输出，未加入 Logic gate；请核对钻孔与阻焊。")
		} else if pad.Drill > 0 {
			stack = append(stack, "-2 "+shape, "-1 "+shape, "0 "+shape)
			for _, entry := range []struct {
				layer string
				n     int
			}{{"F.Mask", 21}, {"B.Mask", 28}} {
				if explicit[entry.layer] {
					stack = append(stack, fmt.Sprintf("%d 0 R", entry.n))
				} else {
					stack = append(stack, fmt.Sprintf("%d %s", entry.n, shape))
				}
			}
		} else if pad.Layer == "B.Cu" {
			stack = append(stack, "-2 0 R", "-1 0 R", "0 "+shape)
			for _, entry := range []struct {
				layer string
				n     int
			}{{"B.Mask", 28}, {"B.Paste", 22}} {
				if explicit[entry.layer] {
					stack = append(stack, fmt.Sprintf("%d 0 R", entry.n))
				} else {
					stack = append(stack, fmt.Sprintf("%d %s", entry.n, shape))
				}
			}
		} else {
			stack = append(stack, "-2 "+shape, "-1 0 R", "0 0 R")
			for _, entry := range []struct {
				layer string
				n     int
			}{{"F.Mask", 21}, {"F.Paste", 23}} {
				if explicit[entry.layer] {
					stack = append(stack, fmt.Sprintf("%d 0 R", entry.n))
				} else {
					stack = append(stack, fmt.Sprintf("%d %s", entry.n, shape))
				}
			}
		}
		fmt.Fprintf(&b, "PAD %d %d %s %s", t.Index, len(stack), plated, padsN(pad.Drill))
		if pad.DrillWidth > pad.Drill && pad.Drill > 0 {
			fmt.Fprintf(&b, " %s %s 0", padsN(math.Mod(-pad.Rotation+360, 180)), padsN(pad.DrillWidth))
			warnings = append(warnings, "长圆孔轴向按统一模型的焊盘旋转输出，请核对钻孔方向。")
		}
		b.WriteByte('\n')
		for _, line := range stack {
			b.WriteString(line + "\n")
		}
	}
	if len(explicit) > 0 {
		warnings = append(warnings, "检测到独立阻焊/锡膏图元；相应面关闭默认焊盘开窗并使用源图元，请核对是否覆盖所有预期焊盘。")
	}
	b.WriteByte('\n')
	return b.String(), unique(warnings), nil
}

func padsPinEndpoint(pin Pin) (Point, int, int) {
	a := pin.Rotation * math.Pi / 180
	body := Point{pin.X + math.Cos(a)*pin.Length, pin.Y + math.Sin(a)*pin.Length}
	length := 200 * .0254
	outer := Point{body.X - math.Cos(a)*length, body.Y - math.Sin(a)*length}
	rtn, mirror := 0, 0
	switch int(math.Round(angle(pin.Rotation))) % 360 {
	case 90:
		rtn, mirror = 90, 2
	case 180:
		rtn, mirror = 0, 2
	case 270:
		rtn, mirror = 90, 0
	}
	return Point{padsMil(outer.X), -padsMil(outer.Y)}, rtn, mirror
}

func renderPADSSchematic(plan padsPlan, stamp string) string {
	var b strings.Builder
	p := plan.Part
	for _, gate := range plan.Gates {
		pieces := []padsPiece{}
		for _, r := range p.Symbol.Rects {
			if unit(r.Unit) != gate.Unit {
				continue
			}
			kind := "CLOSED"
			if r.Fill {
				kind = "COPCLS"
			}
			pieces = append(pieces, padsPiece{Kind: kind, Points: padsClosed(padsYUp([]Point{{r.X, r.Y}, {r.X + r.Width, r.Y}, {r.X + r.Width, r.Y + r.Height}, {r.X, r.Y + r.Height}}, 1/.0254)), Width: 10, Layer: 0})
		}
		for _, line := range p.Symbol.Lines {
			if unit(line.Unit) != gate.Unit {
				continue
			}
			pieces = append(pieces, padsPiece{Kind: "OPEN", Points: padsYUp([]Point{{line.X1, line.Y1}, {line.X2, line.Y2}}, 1/.0254), Width: math.Max(1, padsMil(line.Width)), Layer: 0})
		}
		for _, circle := range p.Symbol.Circles {
			if unit(circle.Unit) != gate.Unit {
				continue
			}
			pieces = append(pieces, padsPiece{Kind: "CIRCLE", Points: padsYUp([]Point{{circle.X - circle.Radius, circle.Y}, {circle.X + circle.Radius, circle.Y}}, 1/.0254), Width: math.Max(1, padsMil(circle.Width)), Layer: 0})
		}
		for _, poly := range p.Symbol.Polygons {
			if unit(poly.Unit) != gate.Unit || len(poly.Points) < 3 {
				continue
			}
			kind := "CLOSED"
			if poly.Fill {
				kind = "COPCLS"
			}
			pieces = append(pieces, padsPiece{Kind: kind, Points: padsClosed(padsYUp(poly.Points, 1/.0254)), Width: math.Max(1, padsMil(poly.Width)), Layer: 0})
		}
		fmt.Fprintf(&b, "%s 0 0 50 5 50 5 2 %d 0 %d 0\nTIMESTAMP %s\n\"Default Font\"\n\"Default Font\"\n", gate.Name, len(pieces), len(gate.Pins), stamp)
		labelY := -padsMil(p.Symbol.Bounds.Y) + 150
		for _, pin := range gate.Pins {
			outer, _, _ := padsPinEndpoint(pin)
			labelY = math.Max(labelY, outer.Y+150)
		}
		fmt.Fprintf(&b, "0 %s 0 4 50 5 \"Default Font\"\nREF-DES\n0 %s 0 4 50 5 \"Default Font\"\nPART-TYPE\n", padsN(labelY), padsN(labelY-75))
		for _, piece := range pieces {
			padsWritePiece(&b, piece)
		}
		for _, pin := range gate.Pins {
			outer, rtn, mirror := padsPinEndpoint(pin)
			fmt.Fprintf(&b, "T%s %s %d %d 140 20 %d 2 230 0 %d 16 PIN\nP0 0 0 0 0 0 0 0 192\n", padsN(outer.X), padsN(outer.Y), rtn, mirror, rtn, rtn)
		}
		b.WriteByte('\n')
	}
	return b.String()
}

func padsPinType(pin Pin) string {
	switch pin.ElectricalType {
	case "input":
		return "L"
	case "output":
		return "S"
	case "bidirectional":
		return "B"
	case "tri_state":
		return "T"
	case "open_collector", "open_emitter":
		return "C"
	case "power_in", "power_out":
		upper := strings.ToUpper(pin.Name)
		if upper == "GND" || upper == "VSS" || strings.HasPrefix(upper, "GND_") || strings.HasPrefix(upper, "VSS_") {
			return "G"
		}
		return "P"
	default:
		return "U"
	}
}
func renderPADSPartType(plan padsPlan, stamp string, symbol, footprint bool) string {
	var b strings.Builder
	p := plan.Part
	pcb := "0"
	if footprint {
		pcb = plan.Name
	}
	family := "TTL"
	switch strings.ToUpper(p.Symbol.Reference) {
	case "R":
		family = "RES"
	case "C":
		family = "CAP"
	case "L":
		family = "IND"
	case "D":
		family = "DIO"
	case "J", "P", "CN":
		family = "CON"
	case "Q":
		family = "TRN"
	}
	attrs := [][2]string{{"Manufacturer_Part_Number", p.Name}, {"Manufacturer", p.Manufacturer}, {"Description", p.Description}, {"Package", p.Package}, {"Source", p.Source + " " + p.SourceURL}, {"Source_Attribution", p.Attribution}}
	if p.LCSCID != "" {
		attrs = append(attrs, [2]string{"LCSC", p.LCSCID})
	}
	pinmap := 0
	if footprint {
		for _, t := range plan.Terminals {
			if t.Physical != strconv.Itoa(t.Index) {
				pinmap = len(plan.Terminals)
				break
			}
		}
	}
	gates := len(plan.Gates)
	if !symbol {
		gates = 1
	}
	fmt.Fprintf(&b, "%s %s I %s %d %d 0 %d 0\nTIMESTAMP %s\n", plan.Name, pcb, family, len(attrs), gates, pinmap, stamp)
	for _, attribute := range attrs {
		fmt.Fprintf(&b, "\"%s\" %s\n", attribute[0], padsASCII(attribute[1], 2047))
	}
	usedNames := map[string]bool{}
	pinLine := func(number, name, typ string) {
		base := padsName(name)
		if base == "COMPONENT" {
			base = padsName("PIN_" + number)
		}
		name = base
		for i := 2; usedNames[name]; i++ {
			name = padsName(fmt.Sprintf("%s_%d", base, i))
		}
		usedNames[name] = true
		fmt.Fprintf(&b, "%s 0 %s %s\n", number, typ, name)
	}
	if symbol {
		for _, gate := range plan.Gates {
			fmt.Fprintf(&b, "GATE 1 %d 0\n%s\n", len(gate.Pins), gate.Name)
			for _, pin := range gate.Pins {
				pinLine(pin.Number, pin.Name, padsPinType(pin))
			}
		}
	} else {
		fmt.Fprintf(&b, "GATE 0 %d 0\n", len(plan.Terminals))
		for _, t := range plan.Terminals {
			pinLine(t.Physical, "PIN_"+t.Physical, "U")
		}
	}
	if pinmap > 0 {
		for i, t := range plan.Terminals {
			if i > 0 {
				b.WriteByte(' ')
			}
			b.WriteString(t.Physical)
		}
		b.WriteByte('\n')
	}
	b.WriteByte('\n')
	return b.String()
}

func padsReadme(name string, symbol, footprint bool, warnings []string) string {
	var b strings.Builder
	b.WriteString("Package Studio — PADS V9 ASCII 导入包\n\n这些是 Parts Library ASCII 交换文件，尚未生成 .pt9/.pd9/.ld9/.ln9 原生库。\n已依据 Mentor PADS 9.4 Library ASCII V9 规格生成；尚未在本机 PADS 中实际回读。\n\n请先新建测试库，不要首次导入到已有生产库。PADS 各版本菜单名称可能略有差异。\n")
	if footprint {
		b.WriteString("1. PADS Layout → File / Library（库管理器）：选中新建库，切换 PCB Decals，Import 导入 " + name + ".d。\n")
	}
	if symbol {
		b.WriteString("2. PADS Logic → File / Library（库管理器）：同一目标库的 CAE Decals / Schematic Decals 中 Import 导入 " + name + ".c。\n   引脚图形引用 PADS 自带标准 PIN（200 mil）；请确保默认 PIN 可用。\n")
	}
	b.WriteString("3. 目标库的 Parts / Part Types 中 Import 导入 " + name + ".p；建议先导入 .d/.c，再导入 .p。\n4. 在 Logic 放置每个 gate，核对物理编号；在 Layout 放置其 PCB decal，核对 pin map、尺寸、孔、图层、阻焊及钢网。\n5. 确认后由 PADS 自身保存库；库目录里的四个同名原生库文件需要一起保管。\n\n坐标：PCB .d 使用 mm，Logic .c 使用 mil，Y 轴转换为向上。\nPCB 端子按 1..N 编号；当原始编号不是该顺序（含 BGA A1 等）时，.p 的 pin-map 表逐项恢复物理编号，不把字母引脚猜成数字。\n多单元符号按 gate 单独生成 CAE decal，gate 内端子次序与 .p 中列表严格一致；原始 pin 信息在 conversion-report.json。\nLogic 使用标准 200 mil PIN；为使其内端仍接到源符号本体，外部端点可能延长。反相/时钟引脚图形尚未专门复刻，隐藏引脚显式显示。\n\n制造图层：先查看 layer-mapping.json；目标 Layout 的层关联才决定制造含义，不应仅凭层号直接生产。\n任意多边形焊盘未实现无损映射时会停止导出，不以外接框补铜。局部阻焊/锡膏扩展未保存在当前统一模型中，默认1:1并保留源显式图元。\nSTEP 附件位于 models（如有），未自动登记到 PADS Reuse / 3DModels；请在实际 3D 视图添加并核对方向、单位和位置。\n\n格式参考（Mentor Graphics 原始规格公开镜像）：\n" + padsFormatSource + "\nPCB/SCH/Part Type 格式分别见第 17–22、12–17、23–27 页。\n\n来源：在线器件来自嘉立创EDA / EasyEDA；内置示例为独立绘制。\nhttps://lceda.cn/\nhttps://easyeda.com/\n")
	if len(warnings) > 0 {
		b.WriteString("\n本批次提示：\n" + strings.Join(unique(warnings), "\n") + "\n")
	}
	return b.String()
}
