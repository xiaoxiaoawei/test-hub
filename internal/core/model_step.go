package core

import (
	"bufio"
	"bytes"
	"errors"
	"fmt"
	"math"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"unicode"
)

// PreparedModelSTEP is a derived copy. The source STEP is never modified. Its
// bounds come from the matching source OBJ mesh, not B-spline control points.
type PreparedModelSTEP struct {
	STEP               []byte
	ExpectedBBoxMM     []float64
	Matrix             [16]float64
	Points, Directions int
}

type stepRecord struct {
	id                     int
	start, end             int
	body, name             string
	args                   []string
	vectorStart, vectorEnd int
	bodyStart              int
}

var stepID = regexp.MustCompile(`^\s*#([0-9]+)\s*=\s*`)
var stepWord = regexp.MustCompile(`(?i)[A-Z_][A-Z_0-9]*`)
var stepReference = regexp.MustCompile(`#([0-9]+)`)

// stepParts splits on commas outside nested lists/strings. Part 21 doubled
// quotes and comments are handled lexically; imported text is never evaluated.
func stepParts(value string) ([]string, error) {
	parts := []string{}
	start, depth := 0, 0
	quote, comment := false, false
	for i := 0; i < len(value); i++ {
		c := value[i]
		if comment {
			if c == '*' && i+1 < len(value) && value[i+1] == '/' {
				comment = false
				i++
			}
			continue
		}
		if quote {
			if c == '\'' {
				if i+1 < len(value) && value[i+1] == '\'' {
					i++
				} else {
					quote = false
				}
			}
			continue
		}
		if c == '/' && i+1 < len(value) && value[i+1] == '*' {
			comment = true
			i++
			continue
		}
		switch c {
		case '\'':
			quote = true
		case '(':
			depth++
		case ')':
			depth--
			if depth < 0 {
				return nil, errors.New("STEP括号不匹配")
			}
		case ',':
			if depth == 0 {
				parts = append(parts, strings.TrimSpace(value[start:i]))
				start = i + 1
			}
		}
	}
	if quote || comment || depth != 0 {
		return nil, errors.New("STEP字符串、注释或括号未闭合")
	}
	parts = append(parts, strings.TrimSpace(value[start:]))
	return parts, nil
}

func stepText(value string, maskStrings bool) string {
	b := []byte(value)
	quote, comment := false, false
	for i := 0; i < len(value); i++ {
		c := value[i]
		if comment {
			b[i] = ' '
			if c == '*' && i+1 < len(value) && value[i+1] == '/' {
				comment = false
				i++
				b[i] = ' '
			}
			continue
		}
		if quote {
			if maskStrings {
				b[i] = ' '
			}
			if c == '\'' {
				if i+1 < len(value) && value[i+1] == '\'' {
					i++
					if maskStrings {
						b[i] = ' '
					}
				} else {
					quote = false
				}
			}
			continue
		}
		if c == '\'' {
			quote = true
			if maskStrings {
				b[i] = ' '
			}
			continue
		}
		if c == '/' && i+1 < len(value) && value[i+1] == '*' {
			comment = true
			b[i] = ' '
			i++
			b[i] = ' '
			continue
		}
	}
	return string(b)
}
func stepCode(value string) string { return strings.ToUpper(stepText(value, true)) }

func readSTEPRecords(raw []byte) ([]stepRecord, error) {
	if len(raw) > 64*1024*1024 {
		return nil, errors.New("STEP模型超过64 MiB限制")
	}
	source := string(raw)
	if !strings.HasPrefix(strings.TrimSpace(source), "ISO-10303-21;") {
		return nil, errors.New("不是STEP Part 21文件")
	}
	records := []stepRecord{}
	seen := map[int]bool{}
	start := 0
	depth := 0
	quote, comment, inData, ended := false, false, false, false
	dataCount := 0
	schemaOK := false
	for i := 0; i < len(source); i++ {
		c := source[i]
		if comment {
			if c == '*' && i+1 < len(source) && source[i+1] == '/' {
				comment = false
				i++
			}
			continue
		}
		if quote {
			if c == '\'' {
				if i+1 < len(source) && source[i+1] == '\'' {
					i++
				} else {
					quote = false
				}
			}
			continue
		}
		if c == '\'' {
			quote = true
			continue
		}
		if c == '/' && i+1 < len(source) && source[i+1] == '*' {
			comment = true
			i++
			continue
		}
		if c == '(' {
			depth++
			if depth > 128 {
				return nil, errors.New("STEP嵌套过深")
			}
		}
		if c == ')' {
			depth--
			if depth < 0 {
				return nil, errors.New("STEP括号不匹配")
			}
		}
		if c != ';' || depth != 0 {
			continue
		}
		rawRecord := source[start:i]
		leading := len(rawRecord) - len(strings.TrimLeftFunc(rawRecord, unicode.IsSpace))
		text := strings.TrimSpace(rawRecord)
		code := strings.TrimSpace(stepCode(text))
		if ended && code != "" {
			return nil, errors.New("STEP结束标记之后存在额外语句")
		}
		if strings.HasPrefix(code, "FILE_SCHEMA") {
			upper := strings.ToUpper(text)
			schemaOK = strings.Contains(upper, "AUTOMOTIVE_DESIGN") || strings.Contains(upper, "CONFIG_CONTROL_DESIGN")
		}
		switch code {
		case "DATA":
			inData = true
			dataCount++
		case "ENDSEC":
			inData = false
		case "END-ISO-10303-21":
			ended = true
		}
		if inData && code != "DATA" {
			match := stepID.FindStringSubmatchIndex(stepCode(text))
			if match == nil {
				return nil, errors.New("STEP DATA只允许显式编号实体")
			}
			id, e := strconv.Atoi(text[match[2]:match[3]])
			if e != nil || id < 1 || seen[id] {
				return nil, errors.New("STEP实体编号无效或重复")
			}
			seen[id] = true
			cleanBody := stepText(text[match[1]:], false)
			left := len(cleanBody) - len(strings.TrimLeftFunc(cleanBody, unicode.IsSpace))
			body := strings.TrimSpace(cleanBody)
			rec := stepRecord{id: id, start: start, end: i + 1, body: body, bodyStart: start + leading + match[1] + left}
			if !strings.HasPrefix(body, "(") {
				open := strings.IndexByte(body, '(')
				if open < 1 || !strings.HasSuffix(body, ")") {
					return nil, errors.New("STEP实体格式无效")
				}
				rec.name = strings.TrimSpace(strings.ToUpper(body[:open]))
				rec.args, e = stepParts(body[open+1 : len(body)-1])
				if e != nil {
					return nil, e
				}
				if (rec.name == "CARTESIAN_POINT" || rec.name == "DIRECTION") && len(rec.args) == 2 {
					pos := strings.LastIndex(body, rec.args[1])
					if pos < 0 {
						return nil, errors.New("STEP坐标位置无效")
					}
					rec.vectorStart = start + leading + match[1] + left + pos
					rec.vectorEnd = rec.vectorStart + len(rec.args[1])
					if strings.Contains(source[rec.vectorStart:rec.vectorEnd], "/*") {
						return nil, errors.New("不支持坐标内部注释，源文件未修改")
					}
				}
			}
			records = append(records, rec)
			if len(records) > 750000 {
				return nil, errors.New("STEP实体数量超过限制")
			}
		}
		start = i + 1
	}
	if quote || comment || depth != 0 || !ended || inData || dataCount != 1 || !schemaOK || len(records) == 0 {
		return nil, errors.New("仅支持完整的单DATA AP203/AP214 STEP模型")
	}
	if strings.TrimSpace(stepCode(source[start:])) != "" {
		return nil, errors.New("STEP含未结束的尾部语句")
	}
	for _, r := range records {
		for _, ref := range stepReference.FindAllStringSubmatch(stepCode(r.body), -1) {
			id, e := strconv.Atoi(ref[1])
			if e != nil || !seen[id] {
				return nil, errors.New("STEP存在未定义的实体引用")
			}
		}
	}
	return records, nil
}

func stepVector(value string) ([]float64, error) {
	value = strings.TrimSpace(value)
	if len(value) < 2 || value[0] != '(' || value[len(value)-1] != ')' {
		return nil, errors.New("STEP坐标列表无效")
	}
	parts, e := stepParts(value[1 : len(value)-1])
	if e != nil {
		return nil, e
	}
	if len(parts) != 2 && len(parts) != 3 {
		return nil, errors.New("STEP只支持2D或3D坐标")
	}
	result := make([]float64, len(parts))
	for i, p := range parts {
		n, e := strconv.ParseFloat(strings.ReplaceAll(strings.ReplaceAll(p, "D", "E"), "d", "e"), 64)
		if e != nil || !modelFinite(n) || math.Abs(n) > 1e9 {
			return nil, errors.New("STEP坐标包含无效或过大数值")
		}
		result[i] = n
	}
	return result, nil
}

func modelFinite(v float64) bool { return !math.IsNaN(v) && !math.IsInf(v, 0) }
func modelPlacement(model Model) ([16]float64, error) {
	var matrix [16]float64
	if model.Prepared {
		return matrix, errors.New("已对齐的STEP不可重复烘焙")
	}
	for _, v := range []float64{model.X, model.Y, model.Z, model.RotationX, model.RotationY, model.RotationZ, model.ScaleX, model.ScaleY, model.ScaleZ, model.SourceWidthMM, model.SourceHeightMM} {
		if !modelFinite(v) {
			return matrix, errors.New("模型位置/旋转/比例必须是有限数值")
		}
	}
	if model.PlacementConvention != "easyeda-std-kicad-v1" {
		return matrix, errors.New("未知模型坐标约定，未自动关联")
	}
	if math.Abs(model.RotationX) > 1e-9 || math.Abs(model.RotationY) > 1e-9 {
		return matrix, errors.New("标准版模型非零X/Y旋转的顺序尚未确认，请先手工核对模型，未猜测对齐")
	}
	for _, s := range []float64{model.ScaleX, model.ScaleY, model.ScaleZ} {
		if s != 0 && math.Abs(s-1) > 1e-9 {
			return matrix, errors.New("自动关联暂不支持缩放STEP模型，未近似修改实体")
		}
	}
	if math.Max(math.Abs(model.X), math.Max(math.Abs(model.Y), math.Abs(model.Z))) > 10000 {
		return matrix, errors.New("模型偏移超过10000mm限制")
	}
	// Legacy Model rotation values were converted for KiCad's clockwise angles.
	// EasyEDA Std specifies positive Z as counterclockwise in the top view.
	if math.Abs(model.RotationZ) > 1000000 {
		return matrix, errors.New("模型旋转角超过支持范围")
	}
	angle := -math.Remainder(model.RotationZ, 360) * (math.Pi / 180)
	c, s := math.Cos(angle), math.Sin(angle)
	matrix = [16]float64{c, -s, 0, model.X, s, c, 0, model.Y, 0, 0, 1, model.Z, 0, 0, 0, 1}
	return matrix, nil
}

// PrepareModelSTEP applies a conservative rigid transform to global 3D geometry
// of a single-product BRep. Assembly/local transform chains and nonunit scaling
// are rejected rather than numerically editing lengths or guessing placement.
func PrepareModelSTEP(step, obj []byte, model Model) (PreparedModelSTEP, error) {
	result := PreparedModelSTEP{}
	matrix, e := modelPlacement(model)
	if e != nil {
		return result, e
	}
	result.Matrix = matrix
	records, e := readSTEPRecords(step)
	if e != nil {
		return result, e
	}
	roots, reps, products := 0, 0, 0
	unit := 0.0
	unsafe := map[string]bool{"SHAPE_REPRESENTATION": true, "SHAPE_REPRESENTATION_RELATIONSHIP": true, "NEXT_ASSEMBLY_USAGE_OCCURRENCE": true, "MAPPED_ITEM": true, "REPRESENTATION_MAP": true, "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION": true, "ITEM_DEFINED_TRANSFORMATION": true, "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION": true, "CARTESIAN_POINT_LIST_3D": true, "POINT_REPLICA": true, "CURVE_REPLICA": true, "SURFACE_REPLICA": true, "SOLID_REPLICA": true}
	for _, r := range records {
		code := stepCode(r.body)
		if r.name == "" && (strings.Contains(code, "CARTESIAN_POINT") || strings.Contains(code, "DIRECTION")) {
			return result, errors.New("暂不支持复合形式的STEP点或方向实体")
		}
		if r.name == "AXIS2_PLACEMENT_3D" && (math.Abs(matrix[0]-1) > 1e-12 || math.Abs(matrix[1]) > 1e-12) {
			if len(r.args) != 4 || strings.TrimSpace(r.args[3]) == "$" {
				return result, errors.New("STEP含隐式3D参考方向，不能安全自动旋转")
			}
		}
		for _, word := range stepWord.FindAllString(code, -1) {
			if unsafe[word] || strings.HasPrefix(word, "CARTESIAN_TRANSFORMATION_OPERATOR") || strings.HasPrefix(word, "TESSELLATED_") {
				return result, fmt.Errorf("STEP含不支持的局部/装配变换 %s，未自动关联", word)
			}
		}
		if r.name == "SHAPE_DEFINITION_REPRESENTATION" {
			roots++
		}
		if r.name == "PRODUCT" {
			products++
		}
		if r.name == "ADVANCED_BREP_SHAPE_REPRESENTATION" {
			reps++
		}
		if r.name != "ADVANCED_BREP_SHAPE_REPRESENTATION" && strings.HasSuffix(r.name, "SHAPE_REPRESENTATION") {
			return result, errors.New("STEP含额外的未支持几何representation")
		}
		if strings.Contains(code, "LENGTH_UNIT") {
			if strings.Contains(code, "CONVERSION_BASED_UNIT") {
				return result, errors.New("自动关联暂不支持英制/自定义STEP单位")
			}
			pos := strings.Index(code, "SI_UNIT")
			if pos < 0 {
				return result, errors.New("STEP长度单位未知")
			}
			args := code[pos+len("SI_UNIT"):]
			open := strings.IndexByte(args, '(')
			close := strings.IndexByte(args, ')')
			if open < 0 || close < open {
				return result, errors.New("STEP单位格式错误")
			}
			fields, e := stepParts(args[open+1 : close])
			if e != nil || len(fields) != 2 || strings.TrimSpace(fields[1]) != ".METRE." {
				return result, errors.New("STEP不是已知米制长度单位")
			}
			scale, ok := map[string]float64{"$": 1000, ".MILLI.": 1, ".CENTI.": 10, ".DECI.": 100, ".MICRO.": .001}[strings.TrimSpace(fields[0])]
			if !ok {
				return result, errors.New("不支持此STEP单位前缀")
			}
			if unit != 0 && unit != scale {
				return result, errors.New("STEP含混合长度单位")
			}
			unit = scale
		}
	}
	if roots != 1 || reps != 1 || products != 1 || unit == 0 {
		return result, errors.New("自动关联仅支持单产品、单BRep表示且单位明确的STEP")
	}
	bounds, e := transformedOBJBounds(obj, matrix)
	if e != nil {
		return result, e
	}
	result.ExpectedBBoxMM = bounds
	if model.SourceWidthMM > 0 || model.SourceHeightMM > 0 {
		rawBounds, e := transformedOBJBounds(obj, [16]float64{1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1})
		if e != nil {
			return result, e
		}
		for i, want := range []float64{model.SourceWidthMM, model.SourceHeightMM} {
			if want > 0 && math.Abs(rawBounds[i+3]-rawBounds[i]-want) > math.Max(.02, want*.001) {
				return result, errors.New("源模型的宽高与STEP配套OBJ不符，可能存在未支持的缩放，未猜测比例")
			}
		}
	}
	edits, e := stepRootFrameEdits(step, records)
	if e != nil {
		return result, e
	}
	for _, r := range records {
		if r.name != "CARTESIAN_POINT" && r.name != "DIRECTION" {
			continue
		}
		if len(r.args) != 2 {
			return result, errors.New("STEP点/方向字段数量无效")
		}
		v, e := stepVector(r.args[1])
		if e != nil {
			return result, e
		}
		if len(v) != 3 {
			continue
		}
		x, y, z := v[0], v[1], v[2]
		v[0] = matrix[0]*x + matrix[1]*y
		v[1] = matrix[4]*x + matrix[5]*y
		v[2] = z
		if r.name == "CARTESIAN_POINT" {
			v[0] += matrix[3] / unit
			v[1] += matrix[7] / unit
			v[2] += matrix[11] / unit
			result.Points++
		} else {
			if x*x+y*y+z*z < 1e-24 {
				return result, errors.New("STEP含零方向向量")
			}
			result.Directions++
		}
		edits = append(edits, stepEdit{r.vectorStart, r.vectorEnd, fmt.Sprintf("(%.15g,%.15g,%.15g)", v[0], v[1], v[2])})
	}
	if result.Points < 4 || result.Directions < 1 {
		return result, errors.New("STEP没有足够的有效3D几何")
	}
	sort.Slice(edits, func(i, j int) bool { return edits[i].start < edits[j].start })
	var out bytes.Buffer
	cursor := 0
	for _, edit := range edits {
		if edit.start < cursor {
			return result, errors.New("STEP变换位置重叠")
		}
		out.Write(step[cursor:edit.start])
		out.WriteString(edit.value)
		cursor = edit.end
	}
	out.Write(step[cursor:])
	result.STEP = out.Bytes()
	return result, nil
}

type stepEdit struct {
	start, end int
	value      string
}

func stepRef(value string) (int, error) {
	value = strings.TrimSpace(value)
	if !strings.HasPrefix(value, "#") {
		return 0, errors.New("STEP实体引用无效")
	}
	n, e := strconv.Atoi(value[1:])
	if e != nil || n < 1 {
		return 0, errors.New("STEP实体引用无效")
	}
	return n, nil
}

// The root representation's coordinate frame is kept at identity. Transforming
// that frame as well as the geometry makes STEP readers cancel the transform.
// Clone it with NEW points/directions: old IDs may be shared by actual surfaces.
func stepRootFrameEdits(raw []byte, records []stepRecord) ([]stepEdit, error) {
	byID := map[int]stepRecord{}
	maxID := 0
	var rep stepRecord
	for _, r := range records {
		byID[r.id] = r
		if r.id > maxID {
			maxID = r.id
		}
		if r.name == "ADVANCED_BREP_SHAPE_REPRESENTATION" {
			rep = r
		}
	}
	for _, r := range records {
		if r.name == "SHAPE_DEFINITION_REPRESENTATION" {
			if len(r.args) != 2 {
				return nil, errors.New("STEP根定义格式无效")
			}
			id, e := stepRef(r.args[1])
			if e != nil || id != rep.id {
				return nil, errors.New("STEP根定义未引用唯一BRep，未猜测表示链")
			}
		}
	}
	if len(rep.args) != 3 {
		return nil, errors.New("STEP根BRep表示格式不支持")
	}
	list := strings.TrimSpace(rep.args[1])
	if len(list) < 2 || list[0] != '(' || list[len(list)-1] != ')' {
		return nil, errors.New("STEP根几何列表无效")
	}
	items, e := stepParts(list[1 : len(list)-1])
	if e != nil {
		return nil, e
	}
	axisIndex := -1
	var axis stepRecord
	for i, item := range items {
		id, e := stepRef(item)
		if e != nil {
			return nil, e
		}
		r := byID[id]
		switch r.name {
		case "AXIS2_PLACEMENT_3D":
			if axisIndex >= 0 {
				return nil, errors.New("STEP有多个根坐标框")
			}
			axisIndex = i
			axis = r
		case "MANIFOLD_SOLID_BREP", "BREP_WITH_VOIDS":
		default:
			return nil, errors.New("STEP根包含未支持的几何表示")
		}
	}
	if axisIndex < 0 || len(axis.args) != 4 {
		return nil, errors.New("STEP缺少明确的3D根坐标框")
	}
	id, e := stepRef(axis.args[1])
	if e != nil {
		return nil, e
	}
	point := byID[id]
	if point.name != "CARTESIAN_POINT" || len(point.args) != 2 {
		return nil, errors.New("STEP根原点无效")
	}
	v, e := stepVector(point.args[1])
	if e != nil || len(v) != 3 {
		return nil, errors.New("STEP根原点不是3D坐标")
	}
	for _, n := range v {
		if math.Abs(n) > 1e-12 {
			return nil, errors.New("STEP根坐标框非零，未猜测局部变换")
		}
	}
	for i, expected := range [][3]float64{{0, 0, 1}, {1, 0, 0}} {
		value := strings.TrimSpace(axis.args[i+2])
		if value == "$" {
			continue
		}
		id, e := stepRef(value)
		if e != nil {
			return nil, e
		}
		r := byID[id]
		if r.name != "DIRECTION" || len(r.args) != 2 {
			return nil, errors.New("STEP根方向无效")
		}
		d, e := stepVector(r.args[1])
		if e != nil || len(d) != 3 {
			return nil, errors.New("STEP根方向不是3D")
		}
		length := math.Sqrt(d[0]*d[0] + d[1]*d[1] + d[2]*d[2])
		if length < 1e-12 {
			return nil, errors.New("STEP根方向为零")
		}
		for j, n := range d {
			if math.Abs(n/length-expected[j]) > 1e-12 {
				return nil, errors.New("STEP根坐标框不是identity，未猜测局部变换")
			}
		}
	}
	pos := strings.LastIndex(rep.body, rep.args[1])
	if pos < 0 {
		return nil, errors.New("STEP根列表位置无效")
	}
	start := rep.bodyStart + pos
	end := start + len(rep.args[1])
	if strings.Contains(string(raw[start:end]), "/*") {
		return nil, errors.New("STEP根列表内部注释暂不支持自动变换")
	}
	if maxID > 2000000000 {
		return nil, errors.New("STEP实体编号超过支持范围")
	}
	items[axisIndex] = fmt.Sprintf("#%d", maxID+4)
	addition := fmt.Sprintf("\n/* Package Studio: fixed root frame; source geometry transformed once. */\n#%d=CARTESIAN_POINT('',(0.,0.,0.));\n#%d=DIRECTION('',(0.,0.,1.));\n#%d=DIRECTION('',(1.,0.,0.));\n#%d=AXIS2_PLACEMENT_3D('',#%d,#%d,#%d);\n", maxID+1, maxID+2, maxID+3, maxID+4, maxID+1, maxID+2, maxID+3)
	return []stepEdit{{start, end, "(" + strings.Join(items, ",") + ")"}, {records[len(records)-1].end, records[len(records)-1].end, addition}}, nil
}

func transformedOBJBounds(obj []byte, m [16]float64) ([]float64, error) {
	if len(obj) > 64*1024*1024 {
		return nil, errors.New("OBJ超过64MiB")
	}
	bounds := []float64{math.Inf(1), math.Inf(1), math.Inf(1), math.Inf(-1), math.Inf(-1), math.Inf(-1)}
	scan := bufio.NewScanner(bytes.NewReader(obj))
	scan.Buffer(make([]byte, 4096), 256*1024)
	count := 0
	for scan.Scan() {
		fields := strings.Fields(scan.Text())
		if len(fields) == 0 || fields[0] != "v" {
			continue
		}
		if len(fields) < 4 {
			return nil, errors.New("源OBJ顶点无效")
		}
		var v [3]float64
		for i := range v {
			value, e := strconv.ParseFloat(fields[i+1], 64)
			if e != nil || !modelFinite(value) || math.Abs(value) > 100000 {
				return nil, errors.New("源OBJ坐标无效")
			}
			v[i] = value
		}
		q := [3]float64{m[0]*v[0] + m[1]*v[1] + m[3], m[4]*v[0] + m[5]*v[1] + m[7], v[2] + m[11]}
		for i := range q {
			if !modelFinite(q[i]) {
				return nil, errors.New("模型变换结果不是有限坐标")
			}
			bounds[i] = math.Min(bounds[i], q[i])
			bounds[i+3] = math.Max(bounds[i+3], q[i])
		}
		count++
		if count > 2000000 {
			return nil, errors.New("OBJ顶点超过限制")
		}
	}
	if e := scan.Err(); e != nil {
		return nil, e
	}
	if count < 4 || bounds[3]-bounds[0] < 1e-8 || bounds[4]-bounds[1] < 1e-8 || bounds[5]-bounds[2] < 1e-8 {
		return nil, errors.New("源OBJ没有有效的三维几何范围")
	}
	return bounds, nil
}
