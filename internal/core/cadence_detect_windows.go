//go:build windows

package core

import (
	"golang.org/x/sys/windows/registry"
	"strings"
)

func cadenceRegistryRoots() []cadenceRoot {
	roots := []cadenceRoot{}
	for _, view := range []uint32{registry.WOW64_64KEY, registry.WOW64_32KEY} {
		for _, hive := range []registry.Key{registry.LOCAL_MACHINE, registry.CURRENT_USER} {
			key, err := registry.OpenKey(hive, `SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`, registry.READ|view)
			if err == nil {
				names, _ := key.ReadSubKeyNames(-1)
				key.Close()
				for _, name := range names {
					k, err := registry.OpenKey(hive, `SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\`+name, registry.READ|view)
					if err != nil {
						continue
					}
					display, _, _ := k.GetStringValue("DisplayName")
					lower := strings.ToLower(display)
					if strings.Contains(lower, "cadence") || strings.Contains(lower, "orcad") || strings.Contains(lower, "allegro") {
						path, _, _ := k.GetStringValue("InstallLocation")
						version, _, _ := k.GetStringValue("DisplayVersion")
						if path != "" {
							roots = append(roots, cadenceRoot{Path: path, Version: version, Source: "uninstall-registry"})
						}
					}
					k.Close()
				}
			}
			var walk func(string, int)
			walk = func(path string, depth int) {
				k, err := registry.OpenKey(hive, path, registry.READ|view)
				if err != nil {
					return
				}
				defer k.Close()
				for _, value := range []string{"CDSROOT", "InstallPath", "InstallDir", "InstallLocation", "RootPath", "Path"} {
					p, _, e := k.GetStringValue(value)
					if e == nil && p != "" {
						roots = append(roots, cadenceRoot{Path: p, Version: path, Source: "cadence-registry"})
					}
				}
				if depth > 0 {
					names, _ := k.ReadSubKeyNames(-1)
					for i, n := range names {
						if i >= 100 {
							break
						}
						walk(path+`\`+n, depth-1)
					}
				}
			}
			walk(`SOFTWARE\Cadence Design Systems`, 3)
			walk(`SOFTWARE\Cadence`, 3)
		}
	}
	return roots
}
