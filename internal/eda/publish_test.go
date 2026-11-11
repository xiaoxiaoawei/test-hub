package eda

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func putFile(t *testing.T, dir, name, value string) string {
	t.Helper()
	p := filepath.Join(dir, name)
	if e := os.WriteFile(p, []byte(value), 0600); e != nil {
		t.Fatal(e)
	}
	return p
}
func readFile(t *testing.T, p string) string {
	t.Helper()
	b, e := os.ReadFile(p)
	if e != nil {
		t.Fatal(e)
	}
	return string(b)
}
func TestPublishBackupAndNew(t *testing.T) {
	root := t.TempDir()
	old := putFile(t, root, "lib.olb", "old")
	staged := putFile(t, root, "staged.olb", "new")
	fresh := putFile(t, root, "staged.psm", "symbol")
	keep := putFile(t, root, "unrelated", "keep")
	hash, _ := HashFile(old)
	backup := filepath.Join(root, "backups")
	dest := filepath.Join(root, "new.psm")
	files, e := PublishUpdates([]FileUpdate{{staged, old, hash}, {fresh, dest, ""}}, backup)
	if e != nil {
		t.Fatal(e)
	}
	if len(files) != 2 || readFile(t, old) != "new" || readFile(t, dest) != "symbol" || readFile(t, keep) != "keep" {
		t.Fatal(files)
	}
	if readFile(t, filepath.Join(backup, "000-lib.olb")) != "old" {
		t.Fatal("bad backup")
	}
	if !strings.Contains(readFile(t, filepath.Join(backup, "manifest.json")), hash) {
		t.Fatal("missing hash")
	}
}
func TestPublishRejectsChangedOrColliding(t *testing.T) {
	root := t.TempDir()
	target := putFile(t, root, "old", "old")
	staged := putFile(t, root, "staged", "new")
	h, _ := HashFile(target)
	putFile(t, root, "old", "edited")
	for _, u := range [][]FileUpdate{{{staged, target, h}}, {{staged, target, ""}}, {{target, target, h}}, {{staged, filepath.Join(root, "fresh"), ""}, {staged, filepath.Join(root, "fresh"), ""}}} {
		if _, e := PublishUpdates(u, filepath.Join(root, "backup")); e == nil {
			t.Fatal("accepted unsafe publication")
		}
	}
	if readFile(t, target) != "edited" {
		t.Fatal("existing changed")
	}
}
func TestPublishRollsBackEntireFamily(t *testing.T) {
	root := t.TempDir()
	a := putFile(t, root, "a", "before-a")
	b := putFile(t, root, "b", "before-b")
	sa := putFile(t, root, "sa", "after-a")
	sb := putFile(t, root, "sb", "after-b")
	ha, _ := HashFile(a)
	hb, _ := HashFile(b)
	calls := 0
	_, e := publishUpdates([]FileUpdate{{sa, a, ha}, {sb, b, hb}}, filepath.Join(root, "backup"), func(from, to string) error {
		calls++
		if calls == 2 {
			return errors.New("simulated locked library")
		}
		return os.Rename(from, to)
	}, os.Link)
	if e == nil {
		t.Fatal("expected error")
	}
	if readFile(t, a) != "before-a" || readFile(t, b) != "before-b" {
		t.Fatal("rollback failed")
	}
}
func TestPublishDetectsConcurrentTargetCreation(t *testing.T) {
	root := t.TempDir()
	a := putFile(t, root, "a", "old")
	sa := putFile(t, root, "sa", "new")
	sb := putFile(t, root, "sb", "fresh")
	b := filepath.Join(root, "b")
	h, _ := HashFile(a)
	_, e := publishUpdates([]FileUpdate{{sa, a, h}, {sb, b, ""}}, filepath.Join(root, "backup"), func(from, to string) error {
		if e := os.WriteFile(b, []byte("external"), 0600); e != nil {
			return e
		}
		return os.Rename(from, to)
	}, os.Link)
	if e == nil {
		t.Fatal("expected collision")
	}
	if readFile(t, a) != "old" || readFile(t, b) != "external" {
		t.Fatal("external edit changed")
	}
}
func TestPublishRefusesRedirect(t *testing.T) {
	root := t.TempDir()
	outside := t.TempDir()
	link := filepath.Join(root, "redirect")
	if e := os.Symlink(outside, link); e != nil {
		t.Skip("symlink privilege unavailable")
	}
	source := putFile(t, root, "stage", "new")
	if _, e := PublishUpdates([]FileUpdate{{source, filepath.Join(link, "target"), ""}}, filepath.Join(root, "backup")); e == nil {
		t.Fatal("accepted redirect")
	}
}

func TestRollbackPreservesExternalEdit(t *testing.T) {
	root := t.TempDir()
	a := putFile(t, root, "a", "old-a")
	b := putFile(t, root, "b", "old-b")
	sa := putFile(t, root, "sa", "new-a")
	sb := putFile(t, root, "sb", "new-b")
	ha, _ := HashFile(a)
	hb, _ := HashFile(b)
	calls := 0
	_, e := publishUpdates([]FileUpdate{{sa, a, ha}, {sb, b, hb}}, filepath.Join(root, "backup"), func(from, to string) error {
		calls++
		if calls == 2 {
			if e := os.WriteFile(a, []byte("external"), 0600); e != nil {
				return e
			}
			return errors.New("locked")
		}
		return os.Rename(from, to)
	}, os.Link)
	if e == nil {
		t.Fatal("expected failure")
	}
	if readFile(t, a) != "external" || readFile(t, b) != "old-b" {
		t.Fatal("lost external edit")
	}
}

func TestPublishRefusesNoHardlinkBeforeMovingOriginal(t *testing.T) {
	root := t.TempDir()
	old := putFile(t, root, "lib", "old")
	source := putFile(t, root, "stage", "new")
	h, _ := HashFile(old)
	moved := false
	_, e := publishUpdates([]FileUpdate{{source, old, h}}, filepath.Join(root, "backup"), func(a, b string) error { moved = true; return os.Rename(a, b) }, func(a, b string) error { return errors.New("filesystem has no hardlinks") })
	if e == nil || moved || readFile(t, old) != "old" {
		t.Fatal("unsupported filesystem moved original")
	}
}
func TestRollbackCanRestoreWhenLinkFailsAfterProbe(t *testing.T) {
	root := t.TempDir()
	old := putFile(t, root, "lib", "old")
	source := putFile(t, root, "stage", "new")
	h, _ := HashFile(old)
	calls := 0
	_, e := publishUpdates([]FileUpdate{{source, old, h}}, filepath.Join(root, "backup"), os.Rename, func(a, b string) error {
		calls++
		if calls > 1 {
			return errors.New("link support lost")
		}
		return os.Link(a, b)
	})
	if e == nil || readFile(t, old) != "old" {
		t.Fatal("fallback failed")
	}
}
