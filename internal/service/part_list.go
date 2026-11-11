package service

import (
	"encoding/csv"
	"errors"
	"fmt"
	"io"
	"path/filepath"
	"regexp"
	"strings"
	"unicode"
	"unicode/utf8"
)

type PartListResult struct {
	FileName   string   `json:"fileName"`
	IDs        []string `json:"ids"`
	Duplicates int      `json:"duplicates"`
	TotalRows  int      `json:"totalRows"`
	Warnings   []string `json:"warnings"`
}

var listCandidate = regexp.MustCompile(`(?i)C[0-9]{2,12}`)

func listIDs(s string) []string {
	out := []string{}
	for _, pos := range listCandidate.FindAllStringIndex(s, -1) {
		// Models and reference designators embedded in a longer token are not C IDs.
		if pos[0] > 0 {
			r, _ := utf8.DecodeLastRuneInString(s[:pos[0]])
			if unicode.IsLetter(r) || unicode.IsDigit(r) || r == '_' || r == '-' {
				continue
			}
		}
		if pos[1] < len(s) {
			r, _ := utf8.DecodeRuneInString(s[pos[1]:])
			if unicode.IsLetter(r) || unicode.IsDigit(r) || r == '_' || r == '-' {
				continue
			}
		}
		out = append(out, strings.ToUpper(s[pos[0]:pos[1]]))
	}
	return out
}

func headerKey(s string) string {
	return strings.Map(func(r rune) rune {
		if unicode.IsSpace(r) || strings.ContainsRune("_-.#()（）:/", r) {
			return -1
		}
		return unicode.ToLower(r)
	}, strings.TrimPrefix(s, "\ufeff"))
}
func lcscHeader(s string) bool {
	switch headerKey(s) {
	case "lcsc", "lcscid", "lcscpart", "lcscpartnumber", "lcscpartno", "lcscnumber", "lcsc料号", "lcsc编号", "立创编号", "立创料号", "立创商城编号", "立创商城料号", "c编号", "c料号", "嘉立创编号", "嘉立创料号":
		return true
	}
	return false
}
func otherHeader(s string) bool {
	switch headerKey(s) {
	case "mpn", "model", "partnumber", "manufacturerpartnumber", "型号", "器件型号", "元件型号", "制造商型号", "厂家型号", "comment", "value", "description", "designator", "designators", "referencedesignators", "refdes", "ref", "refs", "reference", "references", "device", "part", "quantity", "qty", "数量", "位号", "封装", "package", "footprint":
		return true
	}
	return false
}

func (s *Service) ParsePartList(fileName, raw string) (PartListResult, error) {
	return ParsePartList(fileName, raw)
}

// ParsePartList only previews a user-supplied list; fetching is a separate action.
func ParsePartList(fileName, raw string) (PartListResult, error) {
	result := PartListResult{FileName: filepath.Base(fileName), IDs: []string{}, Warnings: []string{}}
	ext := strings.ToLower(filepath.Ext(fileName))
	if ext != ".txt" && ext != ".csv" {
		return result, errors.New("清单仅支持 TXT 或 CSV 文件")
	}
	if len(raw) > 2*1024*1024 {
		return result, errors.New("清单超过 2 MB 限制")
	}
	if !utf8.ValidString(raw) {
		return result, errors.New("清单不是有效文本，请使用 UTF-8 或 GB18030 编码")
	}
	for _, r := range raw {
		if unicode.IsControl(r) && r != '\n' && r != '\r' && r != '\t' {
			return result, errors.New("清单包含不支持的控制字符")
		}
	}
	raw = strings.TrimPrefix(raw, "\ufeff")
	seen := map[string]bool{}
	add := func(ids []string) error {
		for _, id := range ids {
			if seen[id] {
				result.Duplicates++
				continue
			}
			if len(result.IDs) >= 500 {
				return errors.New("清单最多包含 500 个不同的 C 编号，请拆分后导入")
			}
			seen[id] = true
			result.IDs = append(result.IDs, id)
		}
		return nil
	}
	// Delimiter detection ignores quoted separators. TXT with a table header is
	// accepted too, while simple line/token lists remain valid.
	first := ""
	for _, line := range strings.Split(strings.ReplaceAll(raw, "\r\n", "\n"), "\n") {
		if strings.TrimSpace(line) != "" {
			first = line
			break
		}
	}
	counts := map[rune]int{',': 0, ';': 0, '\t': 0}
	quoted := false
	for _, r := range first {
		if r == '"' {
			quoted = !quoted
		}
		if !quoted {
			if _, ok := counts[r]; ok {
				counts[r]++
			}
		}
	}
	delim := rune(',')
	for _, r := range []rune{'\t', ';'} {
		if counts[r] > counts[delim] {
			delim = r
		}
	}
	table := ext == ".csv" || counts[delim] > 0
	rows := [][]string{}
	if table {
		reader := csv.NewReader(strings.NewReader(raw))
		reader.Comma = delim
		reader.FieldsPerRecord = -1
		reader.TrimLeadingSpace = true
		for {
			row, e := reader.Read()
			if e == io.EOF {
				break
			}
			if e != nil {
				return result, fmt.Errorf("CSV 格式错误：%w", e)
			}
			rows = append(rows, row)
		}
	} else {
		for _, line := range strings.Split(strings.ReplaceAll(raw, "\r\n", "\n"), "\n") {
			if strings.TrimSpace(line) != "" {
				rows = append(rows, []string{line})
			}
		}
	}
	col := -1
	hasHeader := false
	start := 0
	if len(rows) > 0 {
		for i, field := range rows[0] {
			if lcscHeader(field) {
				if col >= 0 {
					return result, errors.New("存在多个立创编号列，请保留一个明确的编号列")
				}
				col = i
				hasHeader = true
			} else if otherHeader(field) {
				hasHeader = true
			}
		}
		if hasHeader {
			start = 1
		}
		// A mixed multi-column table without an explicit ID header is ambiguous:
		// C12 can be a capacitor designator. Only a plain all-ID first row is
		// treated as a headerless comma/tab-separated list.
		if table && !hasHeader && len(rows[0]) > 1 {
			for _, field := range rows[0] {
				value := strings.TrimSpace(field)
				if value != "" && !lcscPattern.MatchString(value) {
					hasHeader = true
					start = 1
					break
				}
			}
		}
	}
	invalid := 0
	for _, row := range rows[start:] {
		result.TotalRows++
		if hasHeader && col < 0 {
			continue
		}
		ids := []string{}
		if col >= 0 {
			if col < len(row) {
				value := strings.ToUpper(strings.TrimSpace(row[col]))
				if lcscPattern.MatchString(value) {
					ids = append(ids, value)
				}
			}
		} else {
			for _, field := range row {
				ids = append(ids, listIDs(field)...)
			}
		}
		if len(ids) == 0 {
			invalid++
		}
		if e := add(ids); e != nil {
			return result, e
		}
	}
	if hasHeader && col < 0 {
		result.Warnings = append(result.Warnings, "未找到立创 C 编号列；型号和位号不会自动转换为料号。请添加 LCSC Part # 或立创编号列。")
	} else if invalid > 0 {
		result.Warnings = append(result.Warnings, fmt.Sprintf("%d 行没有有效 C 编号，已忽略。", invalid))
	}
	if len(result.IDs) == 0 && !hasHeader {
		result.Warnings = append(result.Warnings, "没有识别到完整 C 编号，例如 C25804；不支持直接批量导入型号。")
	}
	return result, nil
}
