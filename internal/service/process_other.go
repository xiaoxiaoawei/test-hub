//go:build !windows

package service

import "os/exec"

func hideConsole(cmd *exec.Cmd) {}
