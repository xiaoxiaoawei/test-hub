package eda

import (
	"os"
	"syscall"
)

func isReparsePoint(info os.FileInfo) bool {
	if data, ok := info.Sys().(*syscall.Win32FileAttributeData); ok {
		return data.FileAttributes&syscall.FILE_ATTRIBUTE_REPARSE_POINT != 0
	}
	return false
}
