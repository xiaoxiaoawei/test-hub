package core

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

type cadenceRoot struct{ Path, Version, Source string }

// DetectCadenceTools reads PATH, known roots and (on Windows) installation
// registry keys. RuntimeVerified always remains false: no license checkout or
// EDA process is started by detection.
func DetectCadenceTools() []CadenceCandidate {
	result := []CadenceCandidate{}
	seen := map[string]bool{}
	add := func(product, path, version, source string) {
		absolute, err := filepath.Abs(path)
		if err != nil {
			return
		}
		info, err := os.Stat(absolute)
		if err != nil || !info.Mode().IsRegular() {
			return
		}
		key := strings.ToLower(absolute)
		if seen[key] {
			return
		}
		seen[key] = true
		result = append(result, CadenceCandidate{Product: product, Executable: absolute, VersionHint: version, Source: source, RuntimeVerified: false})
	}
	for _, item := range []struct{ Name, Product string }{{"allegro.exe", "Allegro"}, {"pcb_editor.exe", "Allegro"}, {"Capture.exe", "Capture"}, {"capture.exe", "Capture"}} {
		if p, err := exec.LookPath(item.Name); err == nil {
			add(item.Product, p, "", "PATH")
		}
	}
	roots := cadenceRegistryRoots()
	for _, key := range []string{"CDSROOT", "CDS_INST_DIR", "CADENCE_HOME", "CDSROOTPATH"} {
		if p := os.Getenv(key); p != "" {
			roots = append(roots, cadenceRoot{Path: p, Source: "environment:" + key})
		}
	}
	for _, pattern := range []string{`C:\Cadence\SPB_*`, `C:\Cadence\SPB*`, `C:\Cadence\OrCAD*`, `C:\Program Files\Cadence\*`, `C:\Program Files (x86)\Cadence\*`} {
		matches, _ := filepath.Glob(pattern)
		for _, p := range matches {
			roots = append(roots, cadenceRoot{Path: p, Version: filepath.Base(p), Source: "standard-location"})
		}
	}
	for _, root := range roots {
		for _, item := range []struct{ Product, Path string }{{"Allegro", "tools/bin/allegro.exe"}, {"Allegro", "tools/bin/pcb_editor.exe"}, {"Allegro", "tools/pcb/bin/allegro.exe"}, {"Allegro", "tools/pcb/bin/pcb_editor.exe"}, {"Capture", "tools/bin/Capture.exe"}, {"Capture", "tools/capture/Capture.exe"}, {"Capture", "tools/capture/capture.exe"}} {
			add(item.Product, filepath.Join(root.Path, filepath.FromSlash(item.Path)), root.Version, root.Source)
		}
	}
	return result
}
