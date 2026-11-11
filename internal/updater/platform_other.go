//go:build !windows

package updater

import (
	"errors"
	"os/exec"
	"time"
)

func hideWindow(cmd *exec.Cmd) {}
func waitForProcess(pid int, expectedExe string, timeout time.Duration) error {
	return errors.New("自动更新安装仅支持Windows")
}
func verifyEmbeddedVersion(path, version string) error { return nil }
