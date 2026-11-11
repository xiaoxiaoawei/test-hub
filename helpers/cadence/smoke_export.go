//go:build ignore

// Run explicitly with go run helpers/cadence/smoke_export.go.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"packagestudio/internal/core"
)

func main() {
	input := flag.String("input", "", "Optional JSON array of core.Part, or an object containing parts; defaults to independent demo geometry")
	output := flag.String("out", "", "Absolute parent directory for a new batch")
	library := flag.String("library", "CadenceSmoke", "Library name")
	detect := flag.Bool("detect", false, "Print installation candidates without running EDA")
	symbol := flag.Bool("symbol", true, "Generate Capture Tcl")
	footprint := flag.Bool("footprint", true, "Generate Allegro SKILL and replay scripts")
	flag.Parse()
	if *detect {
		write(core.DetectCadenceTools())
		return
	}
	parts := core.DemoParts()
	if *input != "" {
		data, err := os.ReadFile(*input)
		check(err)
		if err = json.Unmarshal(data, &parts); err != nil {
			var req struct {
				Parts []core.Part `json:"parts"`
			}
			check(json.Unmarshal(data, &req))
			parts = req.Parts
		}
	}
	result, err := core.ExportCadence(parts, *output, *library, *symbol, *footprint)
	check(err)
	write(result)
}
func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
func write(value any) {
	data, err := json.MarshalIndent(value, "", "  ")
	check(err)
	fmt.Println(string(data))
}
