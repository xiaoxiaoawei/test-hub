//go:build !windows

package eda

import (
	"errors"
	"os/exec"
)

func padsHideConsole(*exec.Cmd)   {}
func padsRegistryRoots() []string { return nil }
func padsFileIdentity(string) (padsIdentity, error) {
	return padsIdentity{}, errors.New("Windows PE version metadata unavailable")
}
