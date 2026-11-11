package core

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
)

var unsafeName = regexp.MustCompile(`[^\pL\pN_.-]+`)

func SafeName(s string) string {
	s = strings.Trim(unsafeName.ReplaceAllString(strings.TrimSpace(s), "_"), ". _")
	if len([]rune(s)) > 100 {
		s = string([]rune(s)[:100])
	}
	if s == "" {
		s = "Component"
	}
	switch strings.ToUpper(s) {
	case "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9":
		s = "_" + s
	}
	return s
}
func f(v float64) string {
	if math.Abs(v) < .0000005 {
		v = 0
	}
	return strconv.FormatFloat(v, 'f', 6, 64)
}
func q(s string) string {
	s = strings.ReplaceAll(s, "\x00", "")
	s = strings.ReplaceAll(s, "\\", "\\\\")
	s = strings.ReplaceAll(s, "\"", "\\\"")
	s = strings.ReplaceAll(s, "\r", " ")
	s = strings.ReplaceAll(s, "\n", "\\n")
	return "\"" + s + "\""
}
func unit(u int) int {
	if u < 1 {
		return 1
	}
	return u
}
func angle(v float64) float64 {
	v = math.Mod(v, 360)
	if v < 0 {
		v += 360
	}
	return v
}

// ExportKiCad writes a new self-contained batch directory. Existing user libraries
// are never overwritten or registered globally. A staging directory is published
// only after every requested file has been generated successfully.
func ExportKiCad(parts []Part, outputDirectory, libraryName string, includeSymbol, includeFootprint bool) (ExportResult, error) {
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
	libraryName = SafeName(libraryName)
	for _, p := range parts {
		if err := ValidatePart(p, includeSymbol, includeFootprint); err != nil {
			return result, fmt.Errorf("%s：%w", p.Name, err)
		}
	}
	if err := os.MkdirAll(outputDirectory, 0755); err != nil {
		return result, fmt.Errorf("无法创建导出目录：%w", err)
	}
	stage, err := os.MkdirTemp(outputDirectory, ".packagestudio-")
	if err != nil {
		return result, err
	}
	defer os.RemoveAll(stage)
	suffix := strings.TrimPrefix(filepath.Base(stage), ".packagestudio-")
	destination := filepath.Join(outputDirectory, libraryName+"_"+time.Now().Format("20060102_150405")+"_"+suffix)
	put := func(rel string, content []byte) error {
		path := filepath.Join(stage, rel)
		if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
			return err
		}
		if err := os.WriteFile(path, content, 0644); err != nil {
			return err
		}
		result.Files = append(result.Files, filepath.Join(destination, rel))
		return nil
	}
	used := map[string]bool{}
	var symbols strings.Builder
	symbols.WriteString("(kicad_symbol_lib (version 20211014) (generator packagestudio)\n")
	type audit struct {
		ID          string   `json:"id"`
		Name        string   `json:"name"`
		Entry       string   `json:"entry"`
		Source      string   `json:"source"`
		Attribution string   `json:"attribution"`
		PinCount    int      `json:"pinCount"`
		PadCount    int      `json:"padCount"`
		Warnings    []string `json:"warnings"`
	}
	audits := []audit{}
	for _, p := range parts {
		name := SafeName(p.Name)
		if used[strings.ToLower(name)] {
			name = SafeName(name + "_" + p.ID)
		}
		base := name
		for i := 2; used[strings.ToLower(name)]; i++ {
			name = fmt.Sprintf("%s_%d", base, i)
		}
		used[strings.ToLower(name)] = true
		warnings := append([]string{}, p.Warnings...)
		warnings = append(warnings, pinMappingWarnings(p)...)
		modelReference := ""
		if includeFootprint && p.Model != nil && p.Model.Path != "" {
			ext := strings.ToLower(filepath.Ext(p.Model.Path))
			if ext != ".step" && ext != ".stp" && ext != ".wrl" {
				return result, fmt.Errorf("不支持的 3D 模型文件：%s", ext)
			}
			src, err := os.Open(p.Model.Path)
			if err != nil {
				return result, fmt.Errorf("无法读取 3D 模型：%w", err)
			}
			info, err := src.Stat()
			if err != nil || !info.Mode().IsRegular() || info.Size() > 128*1024*1024 {
				src.Close()
				return result, errors.New("3D 模型必须为不超过 128 MiB 的普通文件")
			}
			modelRel := filepath.Join(libraryName+".3dshapes", name+ext)
			if err = os.MkdirAll(filepath.Dir(filepath.Join(stage, modelRel)), 0755); err != nil {
				src.Close()
				return result, err
			}
			dst, err := os.Create(filepath.Join(stage, modelRel))
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
			modelReference = filepath.ToSlash(filepath.Join(destination, modelRel))
			result.Files = append(result.Files, filepath.Join(destination, modelRel))
			warnings = append(warnings, "3D 模型使用源数据的位置/旋转；请在 KiCad 中核对模型方向与高度。")
		}
		if includeFootprint {
			if err := put(filepath.Join(libraryName+".pretty", name+".kicad_mod"), []byte(renderFootprint(p, name, modelReference))); err != nil {
				return result, err
			}
		}
		if includeSymbol {
			symbols.WriteString(renderSymbol(p, name, libraryName, includeFootprint))
		}
		for _, w := range unique(warnings) {
			result.Warnings = append(result.Warnings, p.Name+"："+w)
		}
		audits = append(audits, audit{p.ID, p.Name, name, p.Source, p.Attribution, len(p.Symbol.Pins), len(p.Footprint.Pads), unique(warnings)})
	}
	symbols.WriteString(")\n")
	if includeSymbol {
		if err := put(libraryName+".kicad_sym", []byte(symbols.String())); err != nil {
			return result, err
		}
	}
	if includeFootprint {
		table := "(fp_lib_table\n  (lib (name " + q(libraryName) + ") (type \"KiCad\") (uri " + q(filepath.ToSlash(filepath.Join(destination, libraryName+".pretty"))) + ") (options \"\") (descr \"Package Studio export\"))\n)\n"
		if err := put("fp-lib-table", []byte(table)); err != nil {
			return result, err
		}
	}
	if includeSymbol {
		table := "(sym_lib_table\n  (lib (name " + q(libraryName) + ") (type \"KiCad\") (uri " + q(filepath.ToSlash(filepath.Join(destination, libraryName+".kicad_sym"))) + ") (options \"\") (descr \"Package Studio export\"))\n)\n"
		if err := put("sym-lib-table", []byte(table)); err != nil {
			return result, err
		}
	}
	report, _ := json.MarshalIndent(map[string]any{"generator": "Package Studio", "format": "KiCad 7+", "parts": audits, "warnings": result.Warnings}, "", "  ")
	if err := put("conversion-report.json", report); err != nil {
		return result, err
	}
	readme := "Package Studio — KiCad 导出\n\n本次导出创建独立批次目录，没有覆盖旧库，也未修改 KiCad 全局设置。\n\n1. KiCad → 首选项 → 管理封装库：添加本目录中的 " + libraryName + ".pretty。库昵称使用 " + libraryName + "。\n2. KiCad → 首选项 → 管理符号库：添加本目录中的 " + libraryName + ".kicad_sym。\n3. 在测试工程放置符号、分配封装，并核对引脚映射、铜焊盘、孔、阻焊及 3D 对齐。\n\nfp-lib-table / sym-lib-table 为当前批次生成的参考表，不应直接覆盖现有项目表。\n3D 引用使用本批次的绝对路径；移动目录后请更新模型路径。\n圆弧和曲线转换为离散线段；不支持的图元详见 conversion-report.json。\n\n在线器件数据来源：嘉立创EDA / EasyEDA 官方库\nhttps://lceda.cn/\nhttps://easyeda.com/\n内置演示器件为 Package Studio 独立绘制。\n\n"
	if len(result.Warnings) > 0 {
		readme += "转换提示：\n" + strings.Join(result.Warnings, "\n") + "\n"
	}
	if err := put("使用说明.txt", []byte(readme)); err != nil {
		return result, err
	}
	if err := os.Rename(stage, destination); err != nil {
		return result, fmt.Errorf("无法发布导出目录：%w", err)
	}
	result.Directory = destination
	result.Warnings = unique(result.Warnings)
	return result, nil
}

func ValidatePart(p Part, symbol, footprint bool) error {
	if symbol && len(p.Symbol.Pins) == 0 {
		return errors.New("没有原理图引脚，请关闭原理图导出选项")
	}
	if footprint && len(p.Footprint.Pads) == 0 {
		return errors.New("没有 PCB 焊盘，请关闭封装导出选项")
	}
	for _, a := range p.Footprint.Pads {
		if a.Width <= 0 || a.Height <= 0 || math.IsNaN(a.X) || math.IsNaN(a.Y) || math.IsInf(a.X, 0) || math.IsInf(a.Y, 0) {
			return errors.New("焊盘尺寸或坐标无效")
		}
		if a.Type == "smd" && a.Layer != "F.Cu" && a.Layer != "B.Cu" {
			return fmt.Errorf("焊盘 %s 不在可支持的铜层", a.Number)
		}
		if a.Drill > math.Max(a.Width, a.Height) && a.Type != "np_thru_hole" {
			return fmt.Errorf("焊盘 %s 孔径大于焊盘尺寸", a.Number)
		}
	}
	return nil
}
func pinMappingWarnings(p Part) []string {
	if len(p.Symbol.Pins) == 0 || len(p.Footprint.Pads) == 0 {
		return nil
	}
	pins, pads := map[string]bool{}, map[string]bool{}
	for _, x := range p.Symbol.Pins {
		pins[x.Number] = true
	}
	for _, x := range p.Footprint.Pads {
		if x.Number != "" && x.Type != "np_thru_hole" {
			pads[x.Number] = true
		}
	}
	missingPads, missingPins := []string{}, []string{}
	for x := range pins {
		if !pads[x] {
			missingPads = append(missingPads, x)
		}
	}
	for x := range pads {
		if !pins[x] {
			missingPins = append(missingPins, x)
		}
	}
	sort.Strings(missingPads)
	sort.Strings(missingPins)
	w := []string{}
	if len(missingPads) > 0 {
		w = append(w, "以下符号引脚没有同号焊盘："+strings.Join(missingPads, ", "))
	}
	if len(missingPins) > 0 {
		w = append(w, "以下焊盘没有同号符号引脚："+strings.Join(missingPins, ", "))
	}
	return w
}

func renderFootprint(p Part, name, model string) string {
	var b strings.Builder
	fmt.Fprintf(&b, "(footprint %s (version 20221018) (generator packagestudio)\n  (layer \"F.Cu\")\n  (descr %s)\n  (tags %s)\n", q(name), q(p.Description), q(p.LCSCID+" "+p.Package))
	through := false
	for _, a := range p.Footprint.Pads {
		if a.Drill > 0 {
			through = true
		}
	}
	if through {
		b.WriteString("  (attr through_hole)\n")
	} else {
		b.WriteString("  (attr smd)\n")
	}
	top := p.Footprint.Bounds.Y - 1.2
	bottom := p.Footprint.Bounds.Y + p.Footprint.Bounds.Height + 1.2
	fmt.Fprintf(&b, "  (fp_text reference \"REF**\" (at 0 %s) (layer \"F.SilkS\") (effects (font (size 1 1) (thickness 0.15))))\n  (fp_text value %s (at 0 %s) (layer \"F.Fab\") (effects (font (size 1 1) (thickness 0.15))))\n", f(top), q(name), f(bottom))
	for _, a := range p.Footprint.Lines {
		fmt.Fprintf(&b, "  (fp_line (start %s %s) (end %s %s) (stroke (width %s) (type default)) (layer %s))\n", f(a.X1), f(a.Y1), f(a.X2), f(a.Y2), f(math.Max(a.Width, .001)), q(a.Layer))
	}
	for _, a := range p.Footprint.Circles {
		fmt.Fprintf(&b, "  (fp_circle (center %s %s) (end %s %s) (stroke (width %s) (type default)) (fill none) (layer %s))\n", f(a.X), f(a.Y), f(a.X+a.Radius), f(a.Y), f(math.Max(a.Width, .001)), q(a.Layer))
	}
	for _, a := range p.Footprint.Polygons {
		if len(a.Points) < 3 {
			continue
		}
		b.WriteString("  (fp_poly (pts")
		for _, v := range a.Points {
			fmt.Fprintf(&b, " (xy %s %s)", f(v.X), f(v.Y))
		}
		fill := "none"
		if a.Fill {
			fill = "solid"
		}
		fmt.Fprintf(&b, ") (stroke (width %s) (type default)) (fill %s) (layer %s))\n", f(a.Width), fill, q(a.Layer))
	}
	for _, a := range p.Footprint.Texts {
		if a.Text == "" {
			continue
		}
		fmt.Fprintf(&b, "  (fp_text user %s (at %s %s %s) (layer %s) (effects (font (size %s %s) (thickness 0.12))))\n", q(a.Text), f(a.X), f(a.Y), f(-a.Rotation), q(a.Layer), f(math.Max(a.Size, .5)), f(math.Max(a.Size, .5)))
	}
	for _, a := range p.Footprint.Pads {
		layers := "\"F.Cu\" \"F.Paste\" \"F.Mask\""
		if a.Layer == "B.Cu" {
			layers = "\"B.Cu\" \"B.Paste\" \"B.Mask\""
		}
		if a.Drill > 0 {
			layers = "\"*.Cu\" \"*.Mask\""
		}
		typ := a.Type
		if typ == "" {
			typ = "smd"
		}
		rotation := -a.Rotation
		width, height := a.Width, a.Height
		if a.Shape == "custom" {
			rotation = 0
			width, height = .01, .01
		}
		fmt.Fprintf(&b, "  (pad %s %s %s (at %s %s %s) (size %s %s)", q(a.Number), typ, a.Shape, f(a.X), f(a.Y), f(rotation), f(width), f(height))
		if a.Drill > 0 {
			if a.DrillWidth > a.Drill {
				fmt.Fprintf(&b, " (drill oval %s %s)", f(a.DrillWidth), f(a.Drill))
			} else {
				fmt.Fprintf(&b, " (drill %s)", f(a.Drill))
			}
		}
		fmt.Fprintf(&b, " (layers %s)", layers)
		if a.Shape == "roundrect" {
			b.WriteString(" (roundrect_rratio 0.15)")
		}
		if a.Shape == "custom" {
			b.WriteString(" (options (clearance outline) (anchor rect)) (primitives (gr_poly (pts")
			for _, v := range a.Points {
				fmt.Fprintf(&b, " (xy %s %s)", f(v.X-a.X), f(v.Y-a.Y))
			}
			b.WriteString(") (width 0) (fill yes)))")
		}
		b.WriteString(")\n")
	}
	if model != "" && p.Model != nil {
		m := p.Model
		sx, sy, sz := m.ScaleX, m.ScaleY, m.ScaleZ
		if sx == 0 {
			sx = 1
		}
		if sy == 0 {
			sy = 1
		}
		if sz == 0 {
			sz = 1
		}
		fmt.Fprintf(&b, "  (model %s (offset (xyz %s %s %s)) (scale (xyz %s %s %s)) (rotate (xyz %s %s %s)))\n", q(model), f(m.X), f(m.Y), f(m.Z), f(sx), f(sy), f(sz), f(m.RotationX), f(m.RotationY), f(m.RotationZ))
	}
	b.WriteString(")\n")
	return b.String()
}

func renderSymbol(p Part, name, library string, footprint bool) string {
	var b strings.Builder
	ref := p.Symbol.Reference
	if ref == "" {
		ref = "U"
	}
	fmt.Fprintf(&b, "  (symbol %s (pin_names (offset 0.508)) (in_bom yes) (on_board yes)\n", q(name))
	propertyID := 0
	prop := func(key, value string, y float64, hidden bool) {
		hide := ""
		if hidden {
			hide = " hide"
		}
		fmt.Fprintf(&b, "    (property %s %s (id %d) (at 0 %s 0) (effects (font (size 1.27 1.27))%s))\n", q(key), q(value), propertyID, f(y), hide)
		propertyID++
	}
	prop("Reference", ref, -p.Symbol.Bounds.Y+2.54, false)
	prop("Value", name, -p.Symbol.Bounds.Y+5.08, false)
	fp := ""
	if footprint {
		fp = library + ":" + name
	}
	prop("Footprint", fp, 0, true)
	prop("Datasheet", p.SourceURL, 0, true)
	prop("Description", p.Description, 0, true)
	if p.LCSCID != "" {
		prop("LCSC", p.LCSCID, 0, true)
	}
	units := p.Symbol.Units
	if units < 1 {
		units = 1
	}
	for u := 1; u <= units; u++ {
		fmt.Fprintf(&b, "    (symbol %s\n", q(fmt.Sprintf("%s_%d_1", name, u)))
		for _, a := range p.Symbol.Rects {
			if unit(a.Unit) != u {
				continue
			}
			fill := "none"
			if a.Fill {
				fill = "background"
			}
			fmt.Fprintf(&b, "      (rectangle (start %s %s) (end %s %s) (stroke (width 0.254) (type default)) (fill (type %s)))\n", f(a.X), f(-a.Y), f(a.X+a.Width), f(-a.Y-a.Height), fill)
		}
		for _, a := range p.Symbol.Lines {
			if unit(a.Unit) != u {
				continue
			}
			fmt.Fprintf(&b, "      (polyline (pts (xy %s %s) (xy %s %s)) (stroke (width %s) (type default)) (fill (type none)))\n", f(a.X1), f(-a.Y1), f(a.X2), f(-a.Y2), f(math.Max(a.Width, .1)))
		}
		for _, a := range p.Symbol.Circles {
			if unit(a.Unit) != u {
				continue
			}
			fmt.Fprintf(&b, "      (circle (center %s %s) (radius %s) (stroke (width %s) (type default)) (fill (type none)))\n", f(a.X), f(-a.Y), f(a.Radius), f(math.Max(a.Width, .1)))
		}
		for _, a := range p.Symbol.Polygons {
			if unit(a.Unit) != u {
				continue
			}
			b.WriteString("      (polyline (pts")
			for _, v := range a.Points {
				fmt.Fprintf(&b, " (xy %s %s)", f(v.X), f(-v.Y))
			}
			if len(a.Points) > 0 {
				fmt.Fprintf(&b, " (xy %s %s)", f(a.Points[0].X), f(-a.Points[0].Y))
			}
			fill := "none"
			if a.Fill {
				fill = "background"
			}
			fmt.Fprintf(&b, ") (stroke (width %s) (type default)) (fill (type %s)))\n", f(math.Max(a.Width, .1)), fill)
		}
		for _, a := range p.Symbol.Pins {
			if unit(a.Unit) != u {
				continue
			}
			electrical := a.ElectricalType
			if !strings.Contains("|input|output|bidirectional|tri_state|passive|free|unspecified|power_in|power_out|open_collector|open_emitter|no_connect|", "|"+electrical+"|") {
				electrical = "unspecified"
			}
			style := a.GraphicStyle
			if style != "inverted" && style != "clock" && style != "inverted_clock" {
				style = "line"
			}
			hide := ""
			if a.Hidden {
				hide = " hide"
			}
			fmt.Fprintf(&b, "      (pin %s %s (at %s %s %s) (length %s)%s (name %s (effects (font (size 1.0 1.0)))) (number %s (effects (font (size 1.0 1.0)))))\n", electrical, style, f(a.X), f(-a.Y), f(angle(-a.Rotation)), f(a.Length), hide, q(a.Name), q(a.Number))
		}
		b.WriteString("    )\n")
	}
	b.WriteString("  )\n")
	return b.String()
}
