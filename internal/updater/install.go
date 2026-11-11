package updater

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

type Plan struct {
	Archive    string   `json:"archive"`
	Manifest   Manifest `json:"manifest"`
	InstallDir string   `json:"installDir"`
	DataDir    string   `json:"dataDir"`
	ParentPID  int      `json:"parentPid"`
	Nonce      string   `json:"nonce"`
	HealthPath string   `json:"healthPath"`
}
type Health struct {
	Version string `json:"version"`
	PID     int    `json:"pid"`
	Nonce   string `json:"nonce"`
}

func within(root, target string) bool {
	r, e := filepath.Rel(root, target)
	return e == nil && r != ".." && !strings.HasPrefix(r, ".."+string(os.PathSeparator)) && !filepath.IsAbs(r)
}
func (c *Client) Prepare(archive, executable, dataDir string, parentPID int) (string, string, error) {
	archive, e := filepath.Abs(archive)
	if e != nil {
		return "", "", e
	}
	if !within(c.CacheDir, archive) || filepath.Base(archive) != ArchiveName {
		return "", "", errors.New("只能安装当前客户端已验证的更新包")
	}
	b, e := os.ReadFile(filepath.Join(filepath.Dir(archive), "manifest.json"))
	if e != nil {
		return "", "", e
	}
	m, e := ParseManifest(b)
	if e != nil {
		return "", "", e
	}
	newer, e := Newer(m.Version, c.CurrentVersion)
	if e != nil || !newer {
		return "", "", errors.New("拒绝安装相同版本或旧版本")
	}
	if e = VerifyArchive(archive, m); e != nil {
		return "", "", e
	}
	if !strings.EqualFold(filepath.Base(executable), "PackageStudio.exe") {
		return "", "", errors.New("当前不是正式安装目录中的 PackageStudio.exe")
	}
	nonceBytes := make([]byte, 32)
	if _, e = rand.Read(nonceBytes); e != nil {
		return "", "", e
	}
	nonce := hex.EncodeToString(nonceBytes)
	plan := Plan{Archive: archive, Manifest: m, InstallDir: filepath.Dir(executable), DataDir: dataDir, ParentPID: parentPID, Nonce: nonce, HealthPath: filepath.Join(filepath.Dir(archive), "startup-"+nonce+".json")}
	planPath := filepath.Join(filepath.Dir(archive), "install-plan.json")
	body, _ := json.MarshalIndent(plan, "", "  ")
	if e = os.WriteFile(planPath, body, 0600); e != nil {
		return "", "", e
	}
	helper := filepath.Join(filepath.Dir(archive), "PackageStudio-updater.exe")
	if e = copyFile(executable, helper); e != nil {
		return "", "", e
	}
	return helper, planPath, nil
}
func copyFile(src, dst string) error {
	f, e := os.Open(src)
	if e != nil {
		return e
	}
	defer f.Close()
	out, e := os.Create(dst)
	if e != nil {
		return e
	}
	_, copyErr := io.Copy(out, f)
	closeErr := out.Close()
	if copyErr != nil {
		return copyErr
	}
	return closeErr
}

type transaction struct {
	root, backup       string
	newFiles, replaced []string
}

var startupHealthTimeout = 45 * time.Second

type rollbackFailure struct {
	cause, restore error
	backup         string
}

func (e *rollbackFailure) Error() string {
	return fmt.Sprintf("%v；恢复旧文件失败（备份：%s）：%v", e.cause, e.backup, e.restore)
}
func (e *rollbackFailure) Unwrap() error { return e.cause }

// Validate existing ancestors before creating anything. In particular, MkdirAll
// must not follow a junction and create directories outside the install root.
func safeDestination(root, rel string, create bool) (string, error) {
	root, err := filepath.Abs(root)
	if err != nil {
		return "", err
	}
	dst := filepath.Join(root, rel)
	if !within(root, dst) {
		return "", errors.New("安装目标路径越界")
	}
	resolvedRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		return "", err
	}
	current := root
	components := strings.Split(filepath.Clean(rel), string(os.PathSeparator))
	for _, component := range components[:len(components)-1] {
		current = filepath.Join(current, component)
		info, err := os.Lstat(current)
		if os.IsNotExist(err) {
			if !create {
				return dst, nil
			}
			if err = os.Mkdir(current, 0755); err != nil {
				return "", err
			}
			info, err = os.Lstat(current)
		}
		if err != nil {
			return "", err
		}
		if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
			return "", errors.New("安装目录含重定向路径或非目录文件")
		}
		resolved, err := filepath.EvalSymlinks(current)
		if err != nil || !within(resolvedRoot, resolved) {
			return "", errors.New("安装目录含重定向路径")
		}
	}
	return dst, nil
}

func (t *transaction) rollback() error {
	var all []string
	for i := len(t.newFiles) - 1; i >= 0; i-- {
		if e := os.Remove(filepath.Join(t.root, t.newFiles[i])); e != nil && !os.IsNotExist(e) {
			all = append(all, e.Error())
		}
	}
	for i := len(t.replaced) - 1; i >= 0; i-- {
		rel := t.replaced[i]
		if e := os.Rename(filepath.Join(t.backup, rel), filepath.Join(t.root, rel)); e != nil {
			all = append(all, e.Error())
		}
	}
	if len(all) > 0 {
		return errors.New(strings.Join(all, "; "))
	}
	return nil
}
func publish(stage, root string, files []string) (*transaction, error) {
	seen := map[string]bool{}
	for _, rel := range files {
		canonical, err := archiveRelative("PackageStudio/" + filepath.ToSlash(rel))
		if err != nil || canonical != rel || !allowedUpdateFile(rel) {
			return nil, errors.New("安装文件路径无效")
		}
		key := strings.ToLower(rel)
		if seen[key] {
			return nil, errors.New("安装文件路径重复")
		}
		seen[key] = true
		if _, err := safeDestination(root, rel, false); err != nil {
			return nil, err
		}
		info, err := os.Lstat(filepath.Join(stage, rel))
		if err != nil {
			return nil, err
		}
		if !info.Mode().IsRegular() {
			return nil, errors.New("待安装项目不是普通文件")
		}
	}
	backup, e := os.MkdirTemp(root, ".packagestudio-backup-")
	if e != nil {
		return nil, e
	}
	tx := &transaction{root: root, backup: backup}
	fail := func(cause error) (*transaction, error) {
		if rollbackErr := tx.rollback(); rollbackErr != nil {
			return nil, &rollbackFailure{cause: cause, restore: rollbackErr, backup: tx.backup}
		}
		return nil, cause
	}
	ordered := append([]string{}, files...)
	sort.Slice(ordered, func(i, j int) bool {
		if ordered[i] == "PackageStudio.exe" {
			return false
		}
		if ordered[j] == "PackageStudio.exe" {
			return true
		}
		return ordered[i] < ordered[j]
	})
	for _, rel := range ordered {
		if !within(root, filepath.Join(root, rel)) {
			return fail(errors.New("安装目标路径越界"))
		}
		dst, e := safeDestination(root, rel, true)
		if e != nil {
			return fail(e)
		}
		if info, e := os.Lstat(dst); e == nil {
			if !info.Mode().IsRegular() {
				return fail(errors.New("安装目标不是普通文件"))
			}
			if e = os.MkdirAll(filepath.Dir(filepath.Join(backup, rel)), 0755); e != nil {
				return fail(e)
			}
			if e = os.Rename(dst, filepath.Join(backup, rel)); e != nil {
				return fail(e)
			}
			tx.replaced = append(tx.replaced, rel)
		} else if !os.IsNotExist(e) {
			return fail(e)
		}
		if e = os.Rename(filepath.Join(stage, rel), dst); e != nil {
			return fail(e)
		}
		tx.newFiles = append(tx.newFiles, rel)
	}
	return tx, nil
}
func Apply(planPath string) (err error) {
	b, e := os.ReadFile(planPath)
	if e != nil {
		return e
	}
	if len(b) > 65536 {
		return errors.New("更新计划过大")
	}
	var p Plan
	if e = json.Unmarshal(b, &p); e != nil {
		return e
	}
	if !filepath.IsAbs(p.InstallDir) || !filepath.IsAbs(p.DataDir) || !hashPattern.MatchString(p.Nonce) || p.ParentPID < 1 || filepath.Dir(p.Archive) != filepath.Dir(planPath) || p.HealthPath != filepath.Join(filepath.Dir(planPath), "startup-"+p.Nonce+".json") || !within(filepath.Join(p.DataDir, "updates"), planPath) || !within(filepath.Join(p.DataDir, "updates"), p.HealthPath) {
		return errors.New("更新计划路径无效")
	}
	defer func() {
		status := map[string]any{"version": p.Manifest.Version, "success": err == nil, "time": time.Now().UTC().Format(time.RFC3339)}
		if err != nil {
			status["error"] = err.Error()
		}
		body, _ := json.MarshalIndent(status, "", "  ")
		_ = os.WriteFile(filepath.Join(filepath.Dir(planPath), "installer-result.json"), body, 0600)
	}()
	if e = VerifyArchive(p.Archive, p.Manifest); e != nil {
		return e
	}
	if e = waitForProcess(p.ParentPID, filepath.Join(p.InstallDir, "PackageStudio.exe"), 60*time.Second); e != nil {
		return e
	}
	// Once the old process is gone, every failure must leave it usable again.
	// This includes staging/extraction failures before a transaction even exists.
	defer func() {
		if err == nil {
			return
		}
		var failedRestore *rollbackFailure
		if errors.As(err, &failedRestore) {
			return
		} // A mixed installation must not be started.
		old := exec.Command(filepath.Join(p.InstallDir, "PackageStudio.exe"), "--data-dir", p.DataDir)
		old.Dir = p.InstallDir
		if restartErr := old.Start(); restartErr != nil {
			err = fmt.Errorf("%w；旧版本重启失败：%v", err, restartErr)
		} else {
			_ = old.Process.Release()
		}
	}()
	stage, e := os.MkdirTemp(p.InstallDir, ".packagestudio-stage-")
	if e != nil {
		return e
	}
	defer os.RemoveAll(stage)
	files, e := extractArchive(p.Archive, stage, p.Manifest)
	if e != nil {
		return e
	}
	tx, e := publish(stage, p.InstallDir, files)
	if e != nil {
		return e
	}
	cmd := exec.Command(filepath.Join(p.InstallDir, "PackageStudio.exe"), "--data-dir", p.DataDir, "--update-health", p.HealthPath, "--update-nonce", p.Nonce)
	cmd.Dir = p.InstallDir
	rollback := func(cause error) error {
		if cmd.Process != nil {
			_ = cmd.Process.Kill()
			_, _ = cmd.Process.Wait()
		}
		if e := tx.rollback(); e != nil {
			return &rollbackFailure{cause: cause, restore: e, backup: tx.backup}
		}
		return cause
	}
	if e = cmd.Start(); e != nil {
		return rollback(e)
	}
	deadline := time.Now().Add(startupHealthTimeout)
	for time.Now().Before(deadline) {
		if body, e := os.ReadFile(p.HealthPath); e == nil {
			var health Health
			if json.Unmarshal(body, &health) == nil && health.Version == p.Manifest.Version && health.PID == cmd.Process.Pid && health.Nonce == p.Nonce {
				_ = cmd.Process.Release()
				return nil
			}
		}
		time.Sleep(200 * time.Millisecond)
	}
	return rollback(errors.New("新版本未通过启动确认，已恢复上一版本"))
}
func WriteHealth(dataDir, path, nonce, version string) error {
	if path == "" {
		return nil
	}
	if !within(filepath.Join(dataDir, "updates"), path) || !hashPattern.MatchString(nonce) || filepath.Base(path) != "startup-"+nonce+".json" {
		return errors.New("更新启动确认路径无效")
	}
	b, _ := json.Marshal(Health{version, os.Getpid(), nonce})
	return os.WriteFile(path, b, 0600)
}
func StartHelper(helper, plan string) error {
	cmd := exec.Command(helper, "--apply-update", plan)
	hideWindow(cmd)
	if e := cmd.Start(); e != nil {
		return e
	}
	return cmd.Process.Release()
}
