package updater

import (
	"archive/zip"
	"bytes"
	"debug/pe"
	"errors"
	"fmt"
	"io"
	"os"
	"path"
	"path/filepath"
	"strings"
	"unicode/utf16"
)

var updateRootFiles = map[string]bool{
	"PackageStudio.exe": true, "VERSION": true, "README.md": true, "使用说明.txt": true,
	"LICENSE": true, "LICENSE.txt": true, "THIRD-PARTY-NOTICES.txt": true,
}

func allowedUpdateFile(rel string) bool {
	rel = filepath.ToSlash(rel)
	return updateRootFiles[rel] || strings.HasPrefix(rel, "adapters/") || strings.HasPrefix(rel, "licenses/")
}

func archiveRelative(name string) (string, error) {
	if strings.ContainsAny(name, "\\:<>\"|?*") || !strings.HasPrefix(name, "PackageStudio/") {
		return "", errors.New("更新包路径不属于封装工坊")
	}
	for _, r := range name {
		if r < 32 {
			return "", errors.New("更新包路径包含控制字符")
		}
	}
	rel := strings.TrimSuffix(strings.TrimPrefix(name, "PackageStudio/"), "/")
	if rel == "" {
		return "", nil
	}
	if path.Clean(rel) != rel || strings.HasPrefix(rel, ".") || strings.Contains(rel, "/../") {
		return "", errors.New("更新包包含无效路径")
	}
	for _, segment := range strings.Split(rel, "/") {
		upper := strings.ToUpper(strings.TrimRight(strings.Split(segment, ".")[0], " "))
		upper = strings.NewReplacer("¹", "1", "²", "2", "³", "3").Replace(upper)
		if segment == "" || len(utf16.Encode([]rune(segment))) > 255 {
			return "", errors.New("更新包文件名无效")
		}
		if strings.HasSuffix(segment, ".") || strings.HasSuffix(segment, " ") || upper == "CON" || upper == "CONIN$" || upper == "CONOUT$" || upper == "PRN" || upper == "AUX" || upper == "NUL" || len(upper) == 4 && (strings.HasPrefix(upper, "COM") || strings.HasPrefix(upper, "LPT")) && upper[3] >= '0' && upper[3] <= '9' {
			return "", errors.New("更新包包含不支持的Windows文件名")
		}
	}
	if !allowedUpdateFile(rel) && rel != "adapters" && rel != "licenses" {
		return "", errors.New("更新包目录不受支持")
	}
	return filepath.FromSlash(rel), nil
}
func VerifyPE(b []byte) error {
	f, e := pe.NewFile(bytes.NewReader(b))
	if e != nil {
		return e
	}
	defer f.Close()
	opt, ok := f.OptionalHeader.(*pe.OptionalHeader64)
	if !ok || f.Machine != pe.IMAGE_FILE_MACHINE_AMD64 || opt.Subsystem != pe.IMAGE_SUBSYSTEM_WINDOWS_GUI || f.Characteristics&pe.IMAGE_FILE_DLL != 0 || f.Characteristics&pe.IMAGE_FILE_EXECUTABLE_IMAGE == 0 {
		return errors.New("更新主程序不是 Windows x64 GUI 程序")
	}
	if opt.SizeOfImage == 0 || opt.AddressOfEntryPoint == 0 {
		return errors.New("更新主程序没有可执行入口")
	}
	entryFound := false
	for _, section := range f.Sections {
		delta := uint64(opt.AddressOfEntryPoint) - uint64(section.VirtualAddress)
		if opt.AddressOfEntryPoint < section.VirtualAddress || delta >= uint64(section.Size) || section.Characteristics&pe.IMAGE_SCN_MEM_EXECUTE == 0 {
			continue
		}
		var instruction [1]byte
		if _, err := section.ReadAt(instruction[:], int64(delta)); err == nil {
			entryFound = true
			break
		}
	}
	if !entryFound {
		return errors.New("更新主程序入口不在有效代码节中")
	}
	return nil
}
func inspectArchive(z *zip.ReadCloser, version string) error {
	if len(z.File) > 600 {
		return errors.New("更新包文件过多")
	}
	seen := map[string]bool{} // value distinguishes directories from ordinary files
	var total uint64
	mainFound, versionFound := false, false
	for _, f := range z.File {
		rel, e := archiveRelative(f.Name)
		if e != nil {
			return e
		}
		if f.Mode().Type() != 0 && f.Mode().Type() != os.ModeDir {
			return errors.New("更新包只允许普通文件和目录")
		}
		if rel == "" {
			if !f.FileInfo().IsDir() {
				return errors.New("更新包根条目必须是目录")
			}
			continue
		}
		key := strings.ToLower(filepath.ToSlash(rel))
		if _, exists := seen[key]; exists {
			return errors.New("更新包包含重复路径")
		}
		isDir := f.FileInfo().IsDir()
		for parent := path.Dir(key); parent != "."; parent = path.Dir(parent) {
			if directory, exists := seen[parent]; exists && !directory {
				return errors.New("更新包文件与父目录冲突")
			}
		}
		if !isDir {
			for existing := range seen {
				if strings.HasPrefix(existing, key+"/") {
					return errors.New("更新包文件与子目录冲突")
				}
			}
		}
		seen[key] = isDir
		if isDir {
			if updateRootFiles[filepath.ToSlash(rel)] {
				return errors.New("更新产品文件不能是目录")
			}
			continue
		}
		if !allowedUpdateFile(rel) {
			return errors.New("更新包文件不受支持")
		}
		total += f.UncompressedSize64
		if f.UncompressedSize64 > 128*1024*1024 || total > 256*1024*1024 {
			return errors.New("更新包解压后过大")
		}
		if rel == "PackageStudio.exe" || rel == "VERSION" {
			r, e := f.Open()
			if e != nil {
				return e
			}
			b, e := io.ReadAll(io.LimitReader(r, 128*1024*1024+1))
			r.Close()
			if e != nil {
				return e
			}
			if uint64(len(b)) != f.UncompressedSize64 {
				return errors.New("更新包条目大小不一致")
			}
			if rel == "PackageStudio.exe" {
				if e = VerifyPE(b); e != nil {
					return e
				}
				mainFound = true
			} else {
				if strings.TrimSpace(string(b)) != version {
					return errors.New("更新包内部版本不一致")
				}
				versionFound = true
			}
		} else {
			r, e := f.Open()
			if e != nil {
				return e
			}
			n, readErr := io.Copy(io.Discard, io.LimitReader(r, int64(f.UncompressedSize64)+1))
			r.Close()
			if readErr != nil {
				return fmt.Errorf("更新包条目校验失败 %s：%w", rel, readErr)
			}
			if uint64(n) != f.UncompressedSize64 {
				return errors.New("更新包条目大小不一致")
			}
		}
	}
	if !mainFound || !versionFound {
		return errors.New("更新包缺少主程序或版本信息")
	}
	return nil
}
func VerifyArchive(file string, m Manifest) error {
	if e := m.Validate(); e != nil {
		return e
	}
	info, e := os.Lstat(file)
	if e != nil {
		return e
	}
	if !info.Mode().IsRegular() {
		return errors.New("更新文件不是普通文件")
	}
	h, size, e := hashFile(file)
	if e != nil {
		return e
	}
	if size != m.Size || h != m.SHA256 {
		return errors.New("更新文件大小或SHA-256校验失败")
	}
	z, e := zip.OpenReader(file)
	if e != nil {
		return e
	}
	defer z.Close()
	return inspectArchive(z, m.Version)
}
func extractArchive(file, dest string, m Manifest) ([]string, error) {
	if e := VerifyArchive(file, m); e != nil {
		return nil, e
	}
	z, e := zip.OpenReader(file)
	if e != nil {
		return nil, e
	}
	defer z.Close()
	files := []string{}
	for _, f := range z.File {
		rel, e := archiveRelative(f.Name)
		if e != nil {
			return nil, e
		}
		if rel == "" || f.FileInfo().IsDir() {
			continue
		}
		target, e := safeDestination(dest, rel, true)
		if e != nil {
			return nil, e
		}
		r, e := f.Open()
		if e != nil {
			return nil, e
		}
		out, e := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0644)
		if e != nil {
			r.Close()
			return nil, e
		}
		n, copyErr := io.Copy(out, io.LimitReader(r, int64(f.UncompressedSize64)+1))
		closeErr := out.Close()
		r.Close()
		if copyErr != nil {
			return nil, copyErr
		}
		if closeErr != nil {
			return nil, closeErr
		}
		if uint64(n) != f.UncompressedSize64 {
			return nil, errors.New("解压条目大小不一致")
		}
		files = append(files, rel)
	}
	if e = verifyEmbeddedVersion(filepath.Join(dest, "PackageStudio.exe"), m.Version); e != nil {
		return nil, fmt.Errorf("新程序版本检查失败：%w", e)
	}
	return files, nil
}
