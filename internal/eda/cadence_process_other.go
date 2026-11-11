//go:build !windows

package eda

import "os/exec"

func cadenceHideConsole(cmd *exec.Cmd) {}
