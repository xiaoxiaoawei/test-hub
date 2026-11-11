package updater

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"
)

const Repository = "https://github.com/xiaoxiaoawei/test-hub"
const FeedURL = "https://raw.githubusercontent.com/xiaoxiaoawei/test-hub/main/updates/package-studio/stable.json"
const ArchiveName = "package-studio-windows-x64.zip"
const MaxArchiveBytes = int64(128 * 1024 * 1024)

type Manifest struct {
	SchemaVersion int    `json:"schema_version"`
	Product       string `json:"product"`
	Version       string `json:"version"`
	DownloadURL   string `json:"download_url"`
	NotesURL      string `json:"notes_url"`
	SHA256        string `json:"sha256"`
	Size          int64  `json:"size"`
	PublishedAt   string `json:"published_at"`
	ReleaseNotes  string `json:"release_notes"`
}
type Info struct {
	CurrentVersion string `json:"currentVersion"`
	LatestVersion  string `json:"latestVersion"`
	Available      bool   `json:"available"`
	ReleaseNotes   string `json:"releaseNotes"`
	PublishedAt    string `json:"publishedAt"`
	DownloadURL    string `json:"downloadUrl"`
	ReleaseURL     string `json:"releaseUrl"`
	SHA256         string `json:"sha256"`
	Size           int64  `json:"size"`
	Message        string `json:"message"`
}
type Download struct {
	Path     string `json:"path"`
	Version  string `json:"version"`
	Verified bool   `json:"verified"`
}
type Client struct {
	HTTP           *http.Client
	CurrentVersion string
	CacheDir       string
}

var versionPattern = regexp.MustCompile(`^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$`)
var hashPattern = regexp.MustCompile(`^[a-f0-9]{64}$`)

func ParseVersion(v string) ([3]int, error) {
	var out [3]int
	if !versionPattern.MatchString(v) {
		return out, errors.New("版本号格式无效")
	}
	for i, s := range strings.Split(v, ".") {
		n, e := strconv.Atoi(s)
		if e != nil || n > 65535 {
			return out, errors.New("版本号超出范围")
		}
		out[i] = n
	}
	return out, nil
}
func Newer(latest, current string) (bool, error) {
	a, e := ParseVersion(latest)
	if e != nil {
		return false, e
	}
	b, e := ParseVersion(current)
	if e != nil {
		return false, e
	}
	for i := 0; i < 3; i++ {
		if a[i] != b[i] {
			return a[i] > b[i], nil
		}
	}
	return false, nil
}
func (m Manifest) Validate() error {
	if _, err := ParseVersion(m.Version); err != nil {
		return err
	}
	tag := "package-studio-v" + m.Version
	if m.SchemaVersion != 1 || m.Product != "package-studio" || m.DownloadURL != Repository+"/releases/download/"+tag+"/"+ArchiveName || m.NotesURL != Repository+"/releases/tag/"+tag || !hashPattern.MatchString(m.SHA256) || m.Size < 512 || m.Size > MaxArchiveBytes {
		return errors.New("更新信息无效：产品、地址或校验数据不匹配")
	}
	if _, err := time.Parse(time.RFC3339, m.PublishedAt); err != nil {
		return errors.New("更新发布时间无效")
	}
	if len(m.ReleaseNotes) > 16000 {
		return errors.New("更新说明过长")
	}
	return nil
}
func ParseManifest(b []byte) (Manifest, error) {
	var m Manifest
	if len(b) > 65536 {
		return m, errors.New("更新信息过大")
	}
	// encoding/json otherwise accepts duplicate keys and case-insensitive aliases.
	// The release feed is a flat schema; require each exact key at most once.
	keys := map[string]bool{"schema_version": true, "product": true, "version": true, "download_url": true, "notes_url": true, "sha256": true, "size": true, "published_at": true, "release_notes": true}
	check := json.NewDecoder(strings.NewReader(string(b)))
	start, err := check.Token()
	if err != nil {
		return m, err
	}
	if start != json.Delim('{') {
		return m, errors.New("更新信息必须为对象")
	}
	seen := map[string]bool{}
	for check.More() {
		token, err := check.Token()
		if err != nil {
			return m, err
		}
		key, ok := token.(string)
		if !ok || !keys[key] {
			return m, errors.New("更新信息包含未知字段")
		}
		if seen[key] {
			return m, errors.New("更新信息包含重复字段")
		}
		seen[key] = true
		var value json.RawMessage
		if err := check.Decode(&value); err != nil {
			return m, err
		}
	}
	if _, err := check.Token(); err != nil {
		return m, err
	}
	if check.Decode(new(any)) != io.EOF {
		return m, errors.New("更新信息包含额外数据")
	}
	d := json.NewDecoder(strings.NewReader(string(b)))
	d.DisallowUnknownFields()
	if err := d.Decode(&m); err != nil {
		return m, err
	}
	if d.Decode(new(any)) != io.EOF {
		return m, errors.New("更新信息包含额外数据")
	}
	return m, m.Validate()
}
func NewClient(version, cacheDir string) *Client {
	return &Client{CurrentVersion: version, CacheDir: cacheDir, HTTP: &http.Client{Timeout: 60 * time.Second, CheckRedirect: func(req *http.Request, via []*http.Request) error {
		if len(via) > 6 || req.URL.Scheme != "https" {
			return errors.New("更新下载重定向无效")
		}
		return nil
	}}}
}
func (c *Client) fetch(ctx context.Context) (Manifest, bool, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", FeedURL, nil)
	if err != nil {
		return Manifest{}, false, err
	}
	req.Header.Set("User-Agent", "PackageStudio/"+c.CurrentVersion)
	req.Header.Set("Cache-Control", "no-cache")
	r, err := c.HTTP.Do(req)
	if err != nil {
		return Manifest{}, false, fmt.Errorf("无法连接更新源：%w", err)
	}
	defer r.Body.Close()
	if r.StatusCode == 404 {
		return Manifest{}, false, nil
	}
	if r.StatusCode != 200 {
		return Manifest{}, false, fmt.Errorf("更新源返回 HTTP %d", r.StatusCode)
	}
	b, err := io.ReadAll(io.LimitReader(r.Body, 65537))
	if err != nil {
		return Manifest{}, false, err
	}
	m, err := ParseManifest(b)
	return m, true, err
}
func (c *Client) Check(ctx context.Context) (Info, error) {
	info := Info{CurrentVersion: c.CurrentVersion}
	m, published, err := c.fetch(ctx)
	if err != nil {
		return info, err
	}
	if !published {
		info.Message = "更新通道尚未发布版本。"
		return info, nil
	}
	newer, err := Newer(m.Version, c.CurrentVersion)
	if err != nil {
		return info, err
	}
	info.LatestVersion = m.Version
	info.Available = newer
	info.ReleaseNotes = m.ReleaseNotes
	info.PublishedAt = m.PublishedAt
	info.DownloadURL = m.DownloadURL
	info.ReleaseURL = m.NotesURL
	info.SHA256 = m.SHA256
	info.Size = m.Size
	if newer {
		info.Message = "发现新版本。"
	} else {
		info.Message = "当前已是最新版本。"
	}
	return info, nil
}
func (c *Client) Download(ctx context.Context) (Download, error) {
	m, published, err := c.fetch(ctx)
	if err != nil {
		return Download{}, err
	}
	if !published {
		return Download{}, errors.New("更新通道尚未发布")
	}
	newer, err := Newer(m.Version, c.CurrentVersion)
	if err != nil {
		return Download{}, err
	}
	if !newer {
		return Download{}, errors.New("当前没有需要下载的新版本")
	}
	dir := filepath.Join(c.CacheDir, m.Version)
	if err = os.MkdirAll(dir, 0700); err != nil {
		return Download{}, err
	}
	path := filepath.Join(dir, ArchiveName)
	if VerifyArchive(path, m) != nil {
		req, err := http.NewRequestWithContext(ctx, "GET", m.DownloadURL, nil)
		if err != nil {
			return Download{}, err
		}
		req.Header.Set("User-Agent", "PackageStudio/"+c.CurrentVersion)
		r, err := c.HTTP.Do(req)
		if err != nil {
			return Download{}, err
		}
		defer r.Body.Close()
		if r.StatusCode != 200 {
			return Download{}, fmt.Errorf("更新下载返回 HTTP %d", r.StatusCode)
		}
		f, err := os.CreateTemp(dir, ".download-*.zip")
		if err != nil {
			return Download{}, err
		}
		tmp := f.Name()
		defer os.Remove(tmp)
		n, copyErr := io.Copy(f, io.LimitReader(r.Body, m.Size+1))
		closeErr := f.Close()
		if copyErr != nil {
			return Download{}, copyErr
		}
		if closeErr != nil {
			return Download{}, closeErr
		}
		if n != m.Size {
			return Download{}, errors.New("下载大小与发布信息不一致")
		}
		if err = VerifyArchive(tmp, m); err != nil {
			return Download{}, err
		}
		if err = os.Rename(tmp, path); err != nil {
			return Download{}, err
		}
	}
	b, _ := json.MarshalIndent(m, "", "  ")
	if err = os.WriteFile(filepath.Join(dir, "manifest.json"), b, 0600); err != nil {
		return Download{}, err
	}
	return Download{Path: path, Version: m.Version, Verified: true}, nil
}
func hashFile(path string) (string, int64, error) {
	f, e := os.Open(path)
	if e != nil {
		return "", 0, e
	}
	defer f.Close()
	h := sha256.New()
	n, e := io.Copy(h, io.LimitReader(f, MaxArchiveBytes+1))
	return hex.EncodeToString(h.Sum(nil)), n, e
}
