package eda

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
)

var publishMu sync.Mutex

func HashFile(path string) (string, error) {
	f, e := os.Open(path)
	if e != nil {
		return "", e
	}
	defer f.Close()
	st, e := f.Stat()
	if e != nil {
		return "", e
	}
	if !st.Mode().IsRegular() {
		return "", fmt.Errorf("不是普通文件：%s", path)
	}
	h := sha256.New()
	if _, e = io.Copy(h, f); e != nil {
		return "", e
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}

// Refuse redirected ancestors as well as file symlinks. Windows junctions can
// appear as ModeIrregular; also inspect the native reparse-point attribute.
func directPath(path string) error {
	if !filepath.IsAbs(path) {
		return fmt.Errorf("必须为绝对路径：%s", path)
	}
	for p := filepath.Clean(path); ; p = filepath.Dir(p) {
		st, e := os.Lstat(p)
		if e != nil && !os.IsNotExist(e) {
			return e
		}
		if e == nil && (st.Mode()&(os.ModeSymlink|os.ModeIrregular) != 0 || isReparsePoint(st)) {
			return fmt.Errorf("不允许符号链接或目录联接：%s", p)
		}
		if filepath.Dir(p) == p {
			break
		}
	}
	return nil
}

func copyExclusive(source, target string) error {
	in, e := os.Open(source)
	if e != nil {
		return e
	}
	defer in.Close()
	out, e := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if e != nil {
		return e
	}
	_, e = io.Copy(out, in)
	if e == nil {
		e = out.Sync()
	}
	closeErr := out.Close()
	if e == nil {
		e = closeErr
	}
	return e
}

type publication struct {
	Update    FileUpdate `json:"update"`
	Backup    string     `json:"backup,omitempty"`
	Prepared  string     `json:"-"`
	Rollback  string     `json:"-"`
	Installed bool       `json:"-"`
	Hash      string     `json:"sha256"`
}

func checkBefore(u FileUpdate) error {
	if e := directPath(u.Destination); e != nil {
		return e
	}
	st, e := os.Lstat(u.Destination)
	if u.BeforeSHA256 == "" {
		if os.IsNotExist(e) {
			return nil
		}
		if e != nil {
			return e
		}
		return fmt.Errorf("目标已存在，未覆盖：%s", u.Destination)
	}
	if e != nil {
		return fmt.Errorf("已有库发生变化：%w", e)
	}
	if !st.Mode().IsRegular() {
		return fmt.Errorf("已有库不是普通文件：%s", u.Destination)
	}
	h, e := HashFile(u.Destination)
	if e != nil {
		return e
	}
	if !strings.EqualFold(h, u.BeforeSHA256) {
		return fmt.Errorf("已有库已被其他程序修改，请关闭 EDA 后重试：%s", u.Destination)
	}
	return nil
}

// PublishUpdates backs up existing files and commits a validated staged family.
// Library readers must be closed during publication. On an ordinary I/O error it
// restores every previous file; backups and the manifest remain for recovery.
// This is a file transaction, not a promise of power-loss atomicity across files.
func PublishUpdates(updates []FileUpdate, backupDir string) ([]string, error) {
	return publishUpdates(updates, backupDir, os.Rename, os.Link)
}

func publishUpdates(updates []FileUpdate, backupDir string, rename, link func(string, string) error) (files []string, err error) {
	publishMu.Lock()
	defer publishMu.Unlock()
	if len(updates) == 0 {
		return nil, errors.New("没有可发布的原生库文件")
	}
	if e := directPath(backupDir); e != nil {
		return nil, e
	}
	seen := map[string]bool{}
	entries := make([]publication, 0, len(updates))
	for _, u := range updates {
		u.StagedPath = filepath.Clean(u.StagedPath)
		u.Destination = filepath.Clean(u.Destination)
		key := strings.ToLower(u.Destination)
		if seen[key] {
			return nil, errors.New("重复的目标库文件")
		}
		seen[key] = true
		if strings.EqualFold(u.StagedPath, u.Destination) {
			return nil, errors.New("暂存文件不能与目标相同")
		}
		if e := directPath(u.StagedPath); e != nil {
			return nil, e
		}
		if e := checkBefore(u); e != nil {
			return nil, e
		}
		h, e := HashFile(u.StagedPath)
		if e != nil {
			return nil, e
		}
		// A backup must never become a file in the target library family.
		rel, e := filepath.Rel(backupDir, u.Destination)
		if e == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(os.PathSeparator)) {
			return nil, errors.New("备份目录不能包含目标库")
		}
		entries = append(entries, publication{Update: u, Hash: h})
	}
	// Use a fresh backup directory: no previous backup can be overwritten.
	if e := os.MkdirAll(filepath.Dir(backupDir), 0700); e != nil {
		return nil, e
	}
	if e := os.Mkdir(backupDir, 0700); e != nil {
		return nil, fmt.Errorf("创建独立备份目录：%w", e)
	}
	defer func() {
		for _, p := range entries {
			if p.Prepared != "" {
				_ = os.Remove(p.Prepared)
			}
		}
	}()
	for i := range entries {
		p := &entries[i]
		if p.Update.BeforeSHA256 != "" {
			p.Backup = filepath.Join(backupDir, fmt.Sprintf("%03d-%s", i, filepath.Base(p.Update.Destination)))
			if e := copyExclusive(p.Update.Destination, p.Backup); e != nil {
				return nil, e
			}
			h, e := HashFile(p.Backup)
			if e != nil {
				return nil, e
			}
			if !strings.EqualFold(h, p.Update.BeforeSHA256) {
				return nil, errors.New("备份时检测到已有库发生变化")
			}
		}
		parent := filepath.Dir(p.Update.Destination)
		if e := directPath(parent); e != nil {
			return nil, e
		}
		if e := os.MkdirAll(parent, 0755); e != nil {
			return nil, e
		}
		f, e := os.CreateTemp(parent, ".package-studio-publish-*")
		if e != nil {
			return nil, e
		}
		p.Prepared = f.Name()
		if e = f.Close(); e != nil {
			return nil, e
		}
		if e = os.Remove(p.Prepared); e != nil {
			return nil, e
		}
		if e = copyExclusive(p.Update.StagedPath, p.Prepared); e != nil {
			return nil, e
		}
		h, e := HashFile(p.Prepared)
		if e != nil {
			return nil, e
		}
		if h != p.Hash {
			return nil, errors.New("暂存原生库在复制时发生变化")
		}
		// Probe exclusive publication support before moving ANY original file.
		// exFAT and some shares do not support hard links; fail with originals
		// untouched instead of discovering this in the middle of the commit.
		probe, e := os.CreateTemp(parent, ".package-studio-link-probe-*")
		if e != nil {
			return nil, e
		}
		probeName := probe.Name()
		e = probe.Close()
		if e == nil {
			e = os.Remove(probeName)
		}
		if e != nil {
			return nil, e
		}
		if e = link(p.Prepared, probeName); e != nil {
			return nil, fmt.Errorf("目标目录不支持安全成套发布，请选择本地 NTFS 目录（已有库未修改）：%w", e)
		}
		if e = os.Remove(probeName); e != nil {
			return nil, e
		}
	}
	manifest, e := json.MarshalIndent(entries, "", "  ")
	if e != nil {
		return nil, e
	}
	mf, e := os.OpenFile(filepath.Join(backupDir, "manifest.json"), os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if e != nil {
		return nil, e
	}
	_, e = mf.Write(manifest)
	if e == nil {
		e = mf.Sync()
	}
	ce := mf.Close()
	if e == nil {
		e = ce
	}
	if e != nil {
		return nil, e
	}
	rollback := func(cause error) error {
		failures := []error{cause}
		for i := len(entries) - 1; i >= 0; i-- {
			p := &entries[i]
			if p.Installed {
				if e := directPath(p.Update.Destination); e != nil {
					failures = append(failures, e)
					continue
				}
				h, e := HashFile(p.Update.Destination)
				if e == nil && h != p.Hash {
					failures = append(failures, fmt.Errorf("回滚时目标已被外部修改，保留当前文件与备份 %s", p.Backup))
					continue
				}
				if e != nil && !os.IsNotExist(e) {
					failures = append(failures, e)
					continue
				}
				if e := os.Remove(p.Update.Destination); e != nil && !os.IsNotExist(e) {
					failures = append(failures, e)
					continue
				}
			}
			if p.Rollback != "" {
				// Never replace a target created by another process during a failed
				// publication. Keep both recovery copies instead of losing its data.
				if e := link(p.Rollback, p.Update.Destination); e != nil {
					// Preserve the original path even if link support disappears
					// during the transaction. Copy uses O_EXCL, never overwrites.
					if ce := copyExclusive(p.Rollback, p.Update.Destination); ce != nil {
						failures = append(failures, fmt.Errorf("回滚失败，保留 %s 和备份 %s：%w", p.Rollback, p.Backup, errors.Join(e, ce)))
					}
				} else if e := os.Remove(p.Rollback); e != nil {
					failures = append(failures, e)
				}
			}
		}
		return errors.Join(failures...)
	}
	for i := range entries {
		p := &entries[i]
		if e := checkBefore(p.Update); e != nil {
			return nil, rollback(e)
		}
		if p.Update.BeforeSHA256 != "" {
			// Reserve a unique sibling name without ever replacing an existing file.
			f, e := os.CreateTemp(filepath.Dir(p.Update.Destination), ".package-studio-rollback-*")
			if e != nil {
				return nil, rollback(e)
			}
			name := f.Name()
			e = f.Close()
			if e == nil {
				e = os.Remove(name)
			}
			if e != nil {
				return nil, rollback(e)
			}
			if e = rename(p.Update.Destination, name); e != nil {
				return nil, rollback(e)
			}
			p.Rollback = name
			// Recheck after acquiring the old file, so an external edit during rename
			// cannot be silently replaced by a stale staging copy.
			h, e := HashFile(name)
			if e != nil {
				return nil, rollback(e)
			}
			if !strings.EqualFold(h, p.Update.BeforeSHA256) {
				return nil, rollback(errors.New("提交时检测到已有库被修改"))
			}
		}
		// Link gives exclusive creation (Rename on Unix replaces an existing target).
		if e := link(p.Prepared, p.Update.Destination); e != nil {
			return nil, rollback(fmt.Errorf("发布目标失败：%w", e))
		}
		p.Installed = true
		files = append(files, p.Update.Destination)
	}
	leftovers := []string{}
	for _, p := range entries {
		if p.Rollback != "" {
			if e := os.Remove(p.Rollback); e != nil {
				leftovers = append(leftovers, p.Rollback+": "+e.Error())
			}
		}
	}
	// Every target is already committed. A leftover recovery sibling is harmless;
	// do not turn successful publication into an error implying originals were
	// restored. The immutable backup and this note identify any extra files.
	if len(leftovers) > 0 {
		_ = os.WriteFile(filepath.Join(backupDir, "cleanup-notes.txt"), []byte(strings.Join(leftovers, "\n")), 0600)
	}
	return files, nil
}
