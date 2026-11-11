package service

import (
	"fmt"
	"reflect"
	"strings"
	"testing"
)

func TestParsePartList(t *testing.T) {
	cases := []struct {
		name, file, raw  string
		ids              []string
		duplicates, rows int
		warning          bool
	}{
		{"plain", "bom.txt", "\ufeffC25804\nc25804\nSTM32F103C8T6\nC42405905", []string{"C25804", "C42405905"}, 1, 4, true},
		{"column", "bom.csv", "Designator,MPN,LCSC Part #\nC12,STM32F103C8T6,C25804\nC44,C555,C42405905", []string{"C25804", "C42405905"}, 0, 2, false},
		{"only models", "bom.csv", "型号,位号\nC25804,C12", []string{}, 0, 1, true},
		{"ref alias", "bom.csv", "Ref,Part\nC12,100nF", []string{}, 0, 1, true},
		{"unknown header", "bom.csv", "Supplier,SKU\nfoo,C123", []string{}, 0, 1, true},
		{"ambiguous columns", "bom.csv", "C12,100nF\nC123,10uF", []string{}, 0, 1, true},
		{"plain id columns", "bom.csv", "C25804,C42405905\nc25804,C23922", []string{"C25804", "C42405905", "C23922"}, 1, 2, false},
		{"quoted", "bom.csv", "Description;立创编号\n\"hello;world\";C25804\n\"two\nlines\";C42405905", []string{"C25804", "C42405905"}, 0, 2, false},
		{"tabs", "bom.txt", "位号\t立创料号\nC12\tC25804", []string{"C25804"}, 0, 1, false},
		{"boundaries", "bom.txt", "STM32C25804 C25804A A_C42405905 XC555\nC25804，C42405905", []string{"C25804", "C42405905"}, 0, 2, true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got, e := ParsePartList(tc.file, tc.raw)
			if e != nil {
				t.Fatal(e)
			}
			if !reflect.DeepEqual(got.IDs, tc.ids) || got.Duplicates != tc.duplicates || got.TotalRows != tc.rows || (len(got.Warnings) > 0) != tc.warning {
				t.Fatalf("%+v", got)
			}
		})
	}
}
func TestParsePartListLimits(t *testing.T) {
	for _, tc := range []struct{ file, raw string }{{"a.json", "C25804"}, {"a.txt", "C25804\x00"}, {"a.csv", "\"unterminated"}, {"a.txt", strings.Repeat("a", 2*1024*1024+1)}, {"a.csv", "立创编号,LCSC Part #\nC12,C34"}} {
		if _, e := ParsePartList(tc.file, tc.raw); e == nil {
			t.Errorf("accepted %q", tc.file)
		}
	}
	var b strings.Builder
	for i := 100; i < 601; i++ {
		fmt.Fprintf(&b, "C%d\n", i)
	}
	if _, e := ParsePartList("a.txt", b.String()); e == nil {
		t.Fatal("accepted >500 IDs")
	}
}
