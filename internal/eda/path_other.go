//go:build !windows

package eda

import "os"

func isReparsePoint(info os.FileInfo) bool { return false }
