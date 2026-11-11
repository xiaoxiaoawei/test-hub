package updater

import (
	"encoding/binary"
	"errors"
	"fmt"
	"golang.org/x/sys/windows"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"time"
	"unsafe"
)

func hideWindow(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{HideWindow: true, CreationFlags: 0x08000000}
}
func waitForProcess(pid int, expectedExe string, timeout time.Duration) error {
	h, e := windows.OpenProcess(windows.SYNCHRONIZE|windows.PROCESS_QUERY_LIMITED_INFORMATION, false, uint32(pid))
	if e == windows.ERROR_INVALID_PARAMETER {
		return nil
	}
	if e != nil {
		return e
	}
	defer windows.CloseHandle(h)
	buf := make([]uint16, 32768)
	size := uint32(len(buf))
	if e = windows.QueryFullProcessImageName(h, 0, &buf[0], &size); e != nil {
		return e
	}
	actual := windows.UTF16ToString(buf[:size])
	if !strings.EqualFold(filepath.Clean(actual), filepath.Clean(expectedExe)) {
		return errors.New("等待退出的程序与安装目录不一致")
	}
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		result, e := windows.WaitForSingleObject(h, 500)
		if e != nil {
			return e
		}
		if result == windows.WAIT_OBJECT_0 {
			return nil
		}
	}
	return errors.New("等待旧程序退出超时，请关闭后重试")
}
func verifyEmbeddedVersion(path, version string) error {
	file, e := windows.UTF16PtrFromString(path)
	if e != nil {
		return e
	}
	dll := windows.NewLazySystemDLL("version.dll")
	getSize := dll.NewProc("GetFileVersionInfoSizeW")
	getInfo := dll.NewProc("GetFileVersionInfoW")
	query := dll.NewProc("VerQueryValueW")
	size, _, _ := getSize.Call(uintptr(unsafe.Pointer(file)), 0)
	if size < 52 || size > 16*1024*1024 {
		return errors.New("程序缺少版本资源")
	}
	data := make([]byte, size)
	ok, _, e := getInfo.Call(uintptr(unsafe.Pointer(file)), 0, size, uintptr(unsafe.Pointer(&data[0])))
	if ok == 0 {
		return fmt.Errorf("无法读取程序版本：%v", e)
	}
	root, _ := windows.UTF16PtrFromString("\\")
	var ptr uintptr
	var length uint32
	ok, _, e = query.Call(uintptr(unsafe.Pointer(&data[0])), uintptr(unsafe.Pointer(root)), uintptr(unsafe.Pointer(&ptr)), uintptr(unsafe.Pointer(&length)))
	if ok == 0 || length < 52 {
		return fmt.Errorf("程序版本结构无效：%v", e)
	}
	fixed := unsafe.Slice((*byte)(unsafe.Pointer(ptr)), 52)
	if binary.LittleEndian.Uint32(fixed) != 0xfeef04bd {
		return errors.New("程序版本签名无效")
	}
	v, e := ParseVersion(version)
	if e != nil {
		return e
	}
	ms := uint32(v[0])<<16 | uint32(v[1])
	ls := uint32(v[2]) << 16
	if binary.LittleEndian.Uint32(fixed[8:]) != ms || binary.LittleEndian.Uint32(fixed[12:]) != ls || binary.LittleEndian.Uint32(fixed[16:]) != ms || binary.LittleEndian.Uint32(fixed[20:]) != ls {
		return errors.New("程序内嵌版本与发布清单不一致")
	}
	return nil
}
