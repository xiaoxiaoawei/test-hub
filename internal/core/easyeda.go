package core

import (
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

const easyUnit = 0.254 // EasyEDA Standard coordinate unit = 10 mil.
var numberPattern = regexp.MustCompile(`[-+]?(?:\d*\.\d+|\d+\.?\d*)(?:[eE][-+]?\d+)?`)
var idPattern = regexp.MustCompile(`(?i)^C[0-9]+$`)
var referencePattern = regexp.MustCompile(`^[A-Za-z]+`)

type object = map[string]any

func obj(v any) object {
	if o, ok := v.(map[string]any); ok {
		return o
	}
	if s, ok := v.(string); ok {
		var o object
		if json.Unmarshal([]byte(s), &o) == nil {
			return o
		}
	}
	return object{}
}
func str(v any) string {
	if v == nil {
		return ""
	}
	switch n := v.(type) {
	case string:
		return n
	case float64:
		return strconv.FormatFloat(n, 'f', -1, 64)
	default:
		return fmt.Sprint(v)
	}
}
func num(v any) float64 {
	n, _ := strconv.ParseFloat(str(v), 64)
	if math.IsNaN(n) || math.IsInf(n, 0) {
		return 0
	}
	return n
}
func at(a []string, i int) string {
	if i >= 0 && i < len(a) {
		return a[i]
	}
	return ""
}
func n(a []string, i int) float64 { return num(at(a, i)) }
func shapes(doc object) []string {
	var s []string
	if a, ok := doc["shape"].([]any); ok {
		for _, v := range a {
			if t, ok := v.(string); ok {
				s = append(s, t)
			}
		}
	}
	return s
}
func dataDoc(o object) object {
	if d, ok := o["dataStr"]; ok {
		return obj(d)
	}
	return o
}
func parameters(doc object) object { return obj(obj(doc["head"])["c_para"]) }

// ParseEasyEDA accepts an EasyEDA Standard library document or products API response.
// It never evaluates source code or any instructions inside imported metadata.
func ParseEasyEDA(raw []byte, requestedID string) (Part, error) {
	p := emptyPart()
	var root object
	if len(raw) > 32*1024*1024 {
		return p, errors.New("JSON 超过 32 MiB 限制")
	}
	if err := json.Unmarshal(raw, &root); err != nil {
		return p, fmt.Errorf("无法读取 JSON：%w", err)
	}
	if success, exists := root["success"]; exists && success == false {
		return p, fmt.Errorf("数据源返回失败：%s", str(root["message"]))
	}
	if r, ok := root["result"]; ok {
		root = obj(r)
	}
	doc := dataDoc(root)
	head := obj(doc["head"])
	cp := parameters(doc)
	if len(doc) == 0 {
		return p, errors.New("JSON 中没有可读取的器件数据")
	}
	p.LCSCID = strings.ToUpper(strings.TrimSpace(requestedID))
	if !idPattern.MatchString(p.LCSCID) {
		p.LCSCID = str(obj(root["lcsc"])["number"])
	}
	if p.LCSCID == "" {
		p.LCSCID = str(cp["Supplier Part"])
	}
	p.Name = str(root["title"])
	if p.Name == "" {
		p.Name = str(cp["name"])
	}
	if p.Name == "" {
		p.Name = str(cp["package"])
	}
	if p.Name == "" {
		p.Name = "ImportedPart"
	}
	p.ID = p.LCSCID
	if p.ID == "" {
		p.ID = str(root["uuid"])
	}
	if p.ID == "" {
		p.ID = str(head["uuid"])
	}
	if p.ID == "" {
		p.ID = SafeName(p.Name)
	}
	p.Description = str(root["description"])
	p.Manufacturer = str(cp["Manufacturer"])
	p.Package = str(cp["package"])
	p.Source = "嘉立创EDA / EasyEDA Standard"
	p.SourceURL = "https://easyeda.com/"
	p.Attribution = "JLCEDA / EasyEDA Official Library · https://lceda.cn/ · https://easyeda.com/"
	p.Symbol.Name = p.Name
	p.Symbol.Reference = referencePattern.FindString(str(cp["pre"]))
	if p.Symbol.Reference == "" {
		p.Symbol.Reference = "U"
	}
	footprintDoc := object{}
	symDocs := []object{}
	typ := str(head["docType"])
	if typ == "4" {
		footprintDoc = doc
	} else if len(shapes(doc)) > 0 {
		symDocs = append(symDocs, doc)
	}
	if subparts, ok := root["subparts"].([]any); ok {
		for _, sub := range subparts {
			d := dataDoc(obj(sub))
			if len(shapes(d)) > 0 {
				symDocs = append(symDocs, d)
			}
		}
	}
	if pd := obj(root["packageDetail"]); len(pd) > 0 {
		footprintDoc = dataDoc(pd)
		if p.Package == "" {
			p.Package = str(pd["title"])
		}
	}
	if len(symDocs) > 0 {
		p.Symbol.Units = len(symDocs)
		for i, d := range symDocs {
			parseSymbol(d, i+1, &p)
		}
	}
	if len(footprintDoc) > 0 {
		parseFootprint(footprintDoc, &p)
	}
	if len(p.Footprint.Pads) == 0 && len(p.Symbol.Pins) == 0 {
		return p, errors.New("未发现引脚或焊盘：目前支持 EasyEDA 标准版元件 JSON，暂不支持专业版工程或整板文件")
	}
	if len(p.Footprint.Pads) == 0 {
		p.Warnings = append(p.Warnings, "源数据未包含 PCB 封装；只能导出原理图符号。")
	}
	if len(p.Symbol.Pins) == 0 {
		p.Warnings = append(p.Warnings, "源数据未包含原理图引脚；只能导出 PCB 封装。")
	}
	if p.Manufacturer == "" && len(symDocs) > 0 {
		p.Manufacturer = str(parameters(symDocs[0])["Manufacturer"])
	}
	if p.Package == "" {
		p.Package = p.Footprint.Name
	}
	p.Warnings = unique(p.Warnings)
	updateBounds(&p)
	return p, nil
}

func parseSymbol(doc object, unit int, p *Part) {
	head := obj(doc["head"])
	ox, oy := num(head["x"]), num(head["y"])
	skipped := map[string]int{}
	for _, raw := range shapes(doc) {
		a := strings.Split(raw, "~")
		switch at(a, 0) {
		case "P":
			groups := strings.Split(raw, "^^")
			h := strings.Split(groups[0], "~")
			if len(h) < 7 {
				skipped["无效引脚"]++
				continue
			}
			name := at(h, 3)
			if len(groups) > 3 {
				name = at(strings.Split(groups[3], "~"), 4)
			}
			direction := math.Mod(180+n(h, 6)+360, 360)
			length := 2.54
			pinX, pinY := n(h, 4), n(h, 5)
			if len(groups) > 2 {
				pts, _ := svgPath(strings.Split(groups[2], "~")[0])
				if len(pts) > 0 && len(pts[0]) >= 2 {
					start := pts[0][0]
					pinX, pinY = start.X, start.Y
					end := pts[0][len(pts[0])-1]
					dx, dy := end.X-start.X, end.Y-start.Y
					length = math.Hypot(dx, dy) * easyUnit
					direction = math.Mod(math.Atan2(dy, dx)*180/math.Pi+360, 360)
				}
			}
			electrical := map[string]string{"0": "unspecified", "1": "input", "2": "output", "3": "bidirectional", "4": "power_in"}[at(h, 2)]
			if electrical == "" {
				electrical = "unspecified"
			}
			physicalNumber := at(h, 3)
			if len(groups) > 4 {
				if physical := at(strings.Split(groups[4], "~"), 4); physical != "" {
					physicalNumber = physical
				}
			}
			style := "line"
			inverted, clock := false, false
			if len(groups) > 5 {
				inverted = at(strings.Split(groups[5], "~"), 0) == "1"
			}
			if len(groups) > 6 {
				clock = at(strings.Split(groups[6], "~"), 0) == "1"
			}
			if inverted && clock {
				style = "inverted_clock"
			} else if inverted {
				style = "inverted"
			} else if clock {
				style = "clock"
			}
			p.Symbol.Pins = append(p.Symbol.Pins, Pin{Number: physicalNumber, Name: name, X: (pinX - ox) * easyUnit, Y: (pinY - oy) * easyUnit, Length: length, Rotation: direction, ElectricalType: electrical, GraphicStyle: style, Hidden: at(h, 1) != "show", Unit: unit})
		case "R":
			p.Symbol.Rects = append(p.Symbol.Rects, Rect{X: (n(a, 1) - ox) * easyUnit, Y: (n(a, 2) - oy) * easyUnit, Width: n(a, 5) * easyUnit, Height: n(a, 6) * easyUnit, Fill: at(a, 10) != "none" && strings.ToLower(at(a, 10)) != "none" && at(a, 10) != "", Unit: unit})
		case "PL", "PG":
			pts := pointPairs(at(a, 1), ox, oy)
			for i := 1; i < len(pts); i++ {
				p.Symbol.Lines = append(p.Symbol.Lines, Line{X1: pts[i-1].X, Y1: pts[i-1].Y, X2: pts[i].X, Y2: pts[i].Y, Width: .254, Unit: unit})
			}
			if at(a, 0) == "PG" && len(pts) > 2 {
				p.Symbol.Polygons = append(p.Symbol.Polygons, Polygon{Points: pts, Width: .254, Fill: strings.ToLower(at(a, 5)) != "none" && at(a, 5) != "", Unit: unit})
			}
		case "E":
			if math.Abs(n(a, 3)-n(a, 4)) < .001 {
				p.Symbol.Circles = append(p.Symbol.Circles, Circle{X: (n(a, 1) - ox) * easyUnit, Y: (n(a, 2) - oy) * easyUnit, Radius: n(a, 3) * easyUnit, Width: .254, Unit: unit})
			} else {
				skipped["椭圆"]++
			}
		case "A", "PT":
			paths, err := svgPath(at(a, 1))
			if err != nil {
				skipped[at(a, 0)]++
				continue
			}
			for _, pts := range paths {
				for i := 1; i < len(pts); i++ {
					p.Symbol.Lines = append(p.Symbol.Lines, Line{X1: (pts[i-1].X - ox) * easyUnit, Y1: (pts[i-1].Y - oy) * easyUnit, X2: (pts[i].X - ox) * easyUnit, Y2: (pts[i].Y - oy) * easyUnit, Width: .254, Unit: unit})
				}
			}
		case "T":
			skipped["符号自由文本"]++ // Name/reference are written as KiCad properties, not duplicated graphic text.
		default:
			skipped[at(a, 0)]++
		}
	}
	for _, key := range sortedKeys(skipped) {
		p.Warnings = append(p.Warnings, fmt.Sprintf("原理图单元 %d：%d 个 %s 图元尚未转换，请回读核对。", unit, skipped[key], key))
	}
}

func parseFootprint(doc object, p *Part) {
	modelsSeen := 0
	head := obj(doc["head"])
	ox, oy := num(head["x"]), num(head["y"])
	p.Footprint.Name = str(parameters(doc)["package"])
	if p.Footprint.Name == "" {
		p.Footprint.Name = p.Package
	}
	if p.Footprint.Name == "" {
		p.Footprint.Name = p.Name
	}
	skipped := map[string]int{}
	for _, raw := range shapes(doc) {
		a := strings.Split(raw, "~")
		switch at(a, 0) {
		case "PAD":
			if len(a) < 12 {
				skipped["无效焊盘"]++
				continue
			}
			shape := map[string]string{"RECT": "rect", "ELLIPSE": "oval", "OVAL": "oval", "POLYGON": "custom"}[strings.ToUpper(at(a, 1))]
			if shape == "" {
				skipped["未知焊盘形状 "+at(a, 1)]++
				continue
			}
			if shape == "oval" && math.Abs(n(a, 4)-n(a, 5)) < .0001 {
				shape = "circle"
			}
			pad := Pad{Number: at(a, 8), X: (n(a, 2) - ox) * easyUnit, Y: (n(a, 3) - oy) * easyUnit, Width: n(a, 4) * easyUnit, Height: n(a, 5) * easyUnit, Shape: shape, Rotation: n(a, 11), Drill: 2 * n(a, 9) * easyUnit, Type: "smd", Layer: layer(at(a, 6))}
			if pad.Drill > 0 {
				pad.Type = "thru_hole"
				if at(a, 15) == "N" {
					pad.Type = "np_thru_hole"
				}
			}
			if n(a, 13) > 2*n(a, 9) && pad.Drill > 0 {
				pad.DrillWidth = n(a, 13) * easyUnit
			}
			if shape == "custom" {
				pad.Points = pointPairs(at(a, 10), ox, oy)
				if len(pad.Points) < 3 {
					skipped["无效多边形焊盘"]++
					continue
				}
			}
			p.Footprint.Pads = append(p.Footprint.Pads, pad)
		case "TRACK":
			pts := pointPairs(at(a, 4), ox, oy)
			for i := 1; i < len(pts); i++ {
				p.Footprint.Lines = append(p.Footprint.Lines, Line{X1: pts[i-1].X, Y1: pts[i-1].Y, X2: pts[i].X, Y2: pts[i].Y, Width: n(a, 1) * easyUnit, Layer: layer(at(a, 2))})
			}
		case "CIRCLE":
			p.Footprint.Circles = append(p.Footprint.Circles, Circle{X: (n(a, 1) - ox) * easyUnit, Y: (n(a, 2) - oy) * easyUnit, Radius: n(a, 3) * easyUnit, Width: n(a, 4) * easyUnit, Layer: layer(at(a, 5))})
		case "ARC":
			paths, err := svgPath(at(a, 4))
			if err != nil {
				skipped["圆弧"]++
				continue
			}
			for _, pts := range paths {
				for i := 1; i < len(pts); i++ {
					p.Footprint.Lines = append(p.Footprint.Lines, Line{X1: (pts[i-1].X - ox) * easyUnit, Y1: (pts[i-1].Y - oy) * easyUnit, X2: (pts[i].X - ox) * easyUnit, Y2: (pts[i].Y - oy) * easyUnit, Width: n(a, 1) * easyUnit, Layer: layer(at(a, 2))})
				}
			}
		case "SOLIDREGION":
			if strings.ToLower(at(a, 4)) == "cutout" {
				skipped["区域挖空（未执行布尔运算）"]++
				continue
			}
			paths, err := svgPath(at(a, 3))
			if err != nil {
				skipped["复杂区域"]++
				continue
			}
			for _, pts := range paths {
				if len(pts) < 3 {
					continue
				}
				for i := range pts {
					pts[i].X = (pts[i].X - ox) * easyUnit
					pts[i].Y = (pts[i].Y - oy) * easyUnit
				}
				p.Footprint.Polygons = append(p.Footprint.Polygons, Polygon{Points: pts, Width: 0, Layer: layer(at(a, 1)), Fill: true})
			}
		case "RECT":
			x, y, w, h := (n(a, 1)-ox)*easyUnit, (n(a, 2)-oy)*easyUnit, n(a, 3)*easyUnit, n(a, 4)*easyUnit
			p.Footprint.Polygons = append(p.Footprint.Polygons, Polygon{Points: []Point{{x, y}, {x + w, y}, {x + w, y + h}, {x, y + h}}, Width: n(a, 6) * easyUnit, Layer: layer(at(a, 5)), Fill: false})
		case "HOLE":
			p.Footprint.Pads = append(p.Footprint.Pads, Pad{X: (n(a, 1) - ox) * easyUnit, Y: (n(a, 2) - oy) * easyUnit, Width: 2 * n(a, 3) * easyUnit, Height: 2 * n(a, 3) * easyUnit, Drill: 2 * n(a, 3) * easyUnit, Shape: "circle", Type: "np_thru_hole", Layer: "*.Cu"})
		case "TEXT": // EasyEDA text format: type,x,y,width,rotation,mirror,layer,...,size,text.
			p.Footprint.Texts = append(p.Footprint.Texts, Text{Text: at(a, 10), X: (n(a, 2) - ox) * easyUnit, Y: (n(a, 3) - oy) * easyUnit, Size: n(a, 9) * easyUnit, Rotation: n(a, 5), Layer: layer(at(a, 7))})
		case "SVGNODE":
			var node object
			if json.Unmarshal([]byte(strings.TrimPrefix(raw, "SVGNODE~")), &node) == nil {
				attrs := obj(node["attrs"])
				uuid := str(attrs["uuid"])
				is3D := str(attrs["c_etype"]) == "outline3D" || str(attrs["layerid"]) == "19" || str(node["layerid"]) == "19"
				if uuid != "" && is3D {
					modelsSeen++
					var warning string
					p.Model, warning = parseSourceModel(attrs, ox, oy)
					if warning != "" {
						p.Warnings = append(p.Warnings, warning)
					}
					if modelsSeen > 1 {
						p.Model.PlacementConvention = "unsupported-multiple-models"
						p.Warnings = append(p.Warnings, "封装包含多个3D模型节点；当前自动关联不合并多个来源，请先核对。")
					}
				}
			}
		default:
			skipped[at(a, 0)]++
		}
	}
	for _, key := range sortedKeys(skipped) {
		p.Warnings = append(p.Warnings, fmt.Sprintf("PCB 封装：%d 个 %s 图元尚未转换，请回读核对。", skipped[key], key))
	}
}

func pointPairs(s string, ox, oy float64) []Point {
	v := numberPattern.FindAllString(s, -1)
	pts := []Point{}
	for i := 0; i+1 < len(v); i += 2 {
		pts = append(pts, Point{(num(v[i]) - ox) * easyUnit, (num(v[i+1]) - oy) * easyUnit})
	}
	return pts
}
func layer(id string) string {
	if l, ok := map[string]string{"1": "F.Cu", "2": "B.Cu", "3": "F.SilkS", "4": "B.SilkS", "5": "F.Paste", "6": "B.Paste", "7": "F.Mask", "8": "B.Mask", "10": "Edge.Cuts", "11": "*.Cu", "12": "Dwgs.User", "13": "F.Fab", "14": "B.Fab", "15": "Cmts.User", "99": "F.Fab", "100": "F.Fab", "101": "F.Fab"}[id]; ok {
		return l
	}
	return "Dwgs.User"
}
func unique(s []string) []string {
	out := []string{}
	seen := map[string]bool{}
	for _, v := range s {
		if !seen[v] {
			seen[v] = true
			out = append(out, v)
		}
	}
	return out
}
func sortedKeys(m map[string]int) []string {
	s := make([]string, 0, len(m))
	for k := range m {
		s = append(s, k)
	}
	sort.Strings(s)
	return s
}
func updateBounds(p *Part) {
	bounds := func(points []Point) Bounds {
		if len(points) == 0 {
			return Bounds{X: -5, Y: -5, Width: 10, Height: 10}
		}
		minx, miny, maxx, maxy := points[0].X, points[0].Y, points[0].X, points[0].Y
		for _, q := range points {
			minx = math.Min(minx, q.X)
			miny = math.Min(miny, q.Y)
			maxx = math.Max(maxx, q.X)
			maxy = math.Max(maxy, q.Y)
		}
		return Bounds{X: minx, Y: miny, Width: math.Max(maxx-minx, .1), Height: math.Max(maxy-miny, .1)}
	}
	fp := []Point{}
	for _, a := range p.Footprint.Pads {
		r := math.Hypot(a.Width, a.Height) / 2
		fp = append(fp, Point{a.X - r, a.Y - r}, Point{a.X + r, a.Y + r})
	}
	for _, a := range p.Footprint.Lines {
		fp = append(fp, Point{a.X1, a.Y1}, Point{a.X2, a.Y2})
	}
	for _, a := range p.Footprint.Circles {
		fp = append(fp, Point{a.X - a.Radius, a.Y - a.Radius}, Point{a.X + a.Radius, a.Y + a.Radius})
	}
	for _, a := range p.Footprint.Polygons {
		fp = append(fp, a.Points...)
	}
	p.Footprint.Bounds = bounds(fp)
	sy := []Point{}
	for _, a := range p.Symbol.Pins {
		sy = append(sy, Point{a.X, a.Y})
	}
	for _, a := range p.Symbol.Rects {
		sy = append(sy, Point{a.X, a.Y}, Point{a.X + a.Width, a.Y + a.Height})
	}
	for _, a := range p.Symbol.Lines {
		sy = append(sy, Point{a.X1, a.Y1}, Point{a.X2, a.Y2})
	}
	p.Symbol.Bounds = bounds(sy)
}
