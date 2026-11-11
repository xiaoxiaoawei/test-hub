package eda

import (
	"os/exec"
	"syscall"
)

func cadenceHideConsole(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{HideWindow: true, CreationFlags: 0x08000000}
}
