package updater

import (
	"os"
	"path/filepath"
	"testing"

	"golang.org/x/sys/windows"
)

func TestPublishRollsBackWhenWindowsDeniesExecutableReplacement(t *testing.T) {
	root, stage := t.TempDir(), t.TempDir()
	for _, entry := range []struct{ root, name, text string }{{root, "PackageStudio.exe", "old-exe"}, {root, "VERSION", "old-version"}, {root, "user.txt", "keep-user"}, {stage, "PackageStudio.exe", "new-exe"}, {stage, "VERSION", "new-version"}, {stage, "licenses/new.txt", "new-license"}} {
		writeFixture(t, entry.root, entry.name, entry.text)
	}
	path, e := windows.UTF16PtrFromString(filepath.Join(root, "PackageStudio.exe"))
	if e != nil {
		t.Fatal(e)
	}
	// No FILE_SHARE_DELETE: this is the same sharing violation encountered when
	// an executable is still in use. The fixture is never launched.
	h, e := windows.CreateFile(path, windows.GENERIC_READ, windows.FILE_SHARE_READ, nil, windows.OPEN_EXISTING, windows.FILE_ATTRIBUTE_NORMAL, 0)
	if e != nil {
		t.Fatal(e)
	}
	defer windows.CloseHandle(h)
	if _, e := publish(stage, root, []string{"PackageStudio.exe", "VERSION", filepath.FromSlash("licenses/new.txt")}); e == nil {
		t.Fatal("locked executable was replaced")
	}
	if contents(t, filepath.Join(root, "VERSION")) != "old-version" || contents(t, filepath.Join(root, "PackageStudio.exe")) != "old-exe" || contents(t, filepath.Join(root, "user.txt")) != "keep-user" {
		t.Fatal("rollback did not preserve previous installation")
	}
	if _, e := os.Stat(filepath.Join(root, "licenses", "new.txt")); !os.IsNotExist(e) {
		t.Fatal("new license was not removed during rollback")
	}
}
