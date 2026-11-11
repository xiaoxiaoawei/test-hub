package eda

import (
	"fmt"
	"os/exec"
	"syscall"
	"unsafe"

	"golang.org/x/sys/windows"
	"golang.org/x/sys/windows/registry"
)

func padsHideConsole(cmd *exec.Cmd) { cmd.SysProcAttr = &syscall.SysProcAttr{HideWindow: true} }
func padsRegistryRoots() []string {
	result := []string{}
	for _, view := range []uint32{registry.WOW64_32KEY, registry.WOW64_64KEY} {
		key, err := registry.OpenKey(registry.LOCAL_MACHINE, `SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`, registry.READ|view)
		if err != nil {
			continue
		}
		names, _ := key.ReadSubKeyNames(-1)
		for _, name := range names {
			sub, e := registry.OpenKey(key, name, registry.QUERY_VALUE|view)
			if e != nil {
				continue
			}
			display, _, _ := sub.GetStringValue("DisplayName")
			if containsPADS(display) {
				path, _, e := sub.GetStringValue("InstallLocation")
				if e == nil && path != "" {
					result = append(result, path)
				}
			}
			sub.Close()
		}
		key.Close()
	}
	return result
}

func padsFileIdentity(path string) (padsIdentity, error) {
	var result padsIdentity
	dll := windows.NewLazySystemDLL("version.dll")
	getSize := dll.NewProc("GetFileVersionInfoSizeW")
	get := dll.NewProc("GetFileVersionInfoW")
	query := dll.NewProc("VerQueryValueW")
	name, err := syscall.UTF16PtrFromString(path)
	if err != nil {
		return result, err
	}
	size, _, _ := getSize.Call(uintptr(unsafe.Pointer(name)), 0)
	if size == 0 || size > 4*1024*1024 {
		return result, fmt.Errorf("PADS 程序缺少 Windows 产品版本元数据")
	}
	buffer := make([]byte, size)
	if ok, _, _ := get.Call(uintptr(unsafe.Pointer(name)), 0, size, uintptr(unsafe.Pointer(&buffer[0]))); ok == 0 {
		return result, fmt.Errorf("无法读取 PADS Windows 版本元数据")
	}
	read := func(key string) (unsafe.Pointer, uint32) {
		keyPtr, _ := syscall.UTF16PtrFromString(key)
		var ptr unsafe.Pointer
		var length uint32
		ok, _, _ := query.Call(uintptr(unsafe.Pointer(&buffer[0])), uintptr(unsafe.Pointer(keyPtr)), uintptr(unsafe.Pointer(&ptr)), uintptr(unsafe.Pointer(&length)))
		if ok == 0 {
			return nil, 0
		}
		base := uintptr(unsafe.Pointer(&buffer[0]))
		address := uintptr(ptr)
		if address < base || address >= base+uintptr(len(buffer)) || uintptr(length) > uintptr(len(buffer))-(address-base) {
			return nil, 0
		}
		return ptr, length
	}
	translation, length := read(`\VarFileInfo\Translation`)
	language, codepage := uint16(0x0409), uint16(0x04b0)
	if translation != nil && length >= 4 {
		words := unsafe.Slice((*uint16)(translation), 2)
		language, codepage = words[0], words[1]
	}
	text := func(key string) string {
		ptr, n := read(fmt.Sprintf(`\StringFileInfo\%04x%04x\%s`, language, codepage, key))
		base := uintptr(unsafe.Pointer(&buffer[0]))
		address := uintptr(ptr)
		if ptr == nil || n == 0 || n > 65536 || address < base || address >= base+uintptr(len(buffer)) || uintptr(n)*2 > uintptr(len(buffer))-(address-base) {
			return ""
		}
		return syscall.UTF16ToString(unsafe.Slice((*uint16)(ptr), n))
	}
	result.Product = text("ProductName")
	result.Description = text("FileDescription")
	result.Version = text("ProductVersion")
	if result.Version == "" {
		result.Version = text("FileVersion")
	}
	return result, nil
}
