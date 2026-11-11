package eda

import (
	"context"
	"crypto/rand"
	"debug/pe"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strings"
	"time"

	"packagestudio/internal/core"
)

var padsLibraryExtensions = []string{".pt9", ".pd9", ".ld9", ".ln9"}

type padsSDK struct {
	Root    string            `json:"root"`
	Factory string            `json:"factory"`
	Layout  string            `json:"layout"`
	Logic   string            `json:"logic"`
	Hashes  map[string]string `json:"hashes"`
}

type padsItems struct {
	Parts    []string `json:"parts"`
	Decals   []string `json:"decals"`
	Symbols  []string `json:"symbols"`
	Drawings []string `json:"drawings"`
}

type padsLibraryJob struct {
	ID             string    `json:"id"`
	Base           string    `json:"base"`
	Existing       bool      `json:"existing"`
	Expected       padsItems `json:"expected"`
	PCB            string    `json:"pcb"`
	Schematic      string    `json:"schematic"`
	PartTypes      string    `json:"partTypes"`
	PartTypeEngine string    `json:"partTypeEngine"`
}

type padsJob struct {
	Protocol  int              `json:"protocol"`
	Nonce     string           `json:"nonce"`
	Phase     string           `json:"phase"`
	Root      string           `json:"root"`
	SDK       padsSDK          `json:"sdk"`
	Libraries []padsLibraryJob `json:"libraries"`
}

type padsLibraryEvidence struct {
	ID       string    `json:"id"`
	Base     string    `json:"base"`
	Before   padsItems `json:"before"`
	After    padsItems `json:"after"`
	Reopened bool      `json:"reopened"`
}

type padsEvidence struct {
	Protocol        int                   `json:"protocol"`
	Nonce           string                `json:"nonce"`
	Phase           string                `json:"phase"`
	Root            string                `json:"root"`
	Engine          string                `json:"engine"`
	NativeGenerated bool                  `json:"nativeGenerated"`
	RuntimeVerified bool                  `json:"runtimeVerified"`
	Libraries       []padsLibraryEvidence `json:"libraries"`
	Warnings        []string              `json:"warnings"`
}

type padsDependencies struct {
	inspect func(string, bool, bool) (padsSDK, error)
	run     func(context.Context, padsJob) (padsEvidence, error)
	publish func([]FileUpdate, string) ([]string, error)
}

// ExportPADS never treats interchange files as native libraries. The selected
// local SDK must import into copies and a separate SDK process must reopen them
// before the cross-file publication transaction is permitted.
func ExportPADS(ctx context.Context, req Request) (core.ExportResult, error) {
	return exportPADSNative(ctx, req, padsDependencies{inspect: inspectPADSSDK, run: padsRunJob, publish: PublishUpdates})
}

func exportPADSNative(ctx context.Context, req Request, deps padsDependencies) (result core.ExportResult, returnErr error) {
	result = core.ExportResult{Warnings: []string{"PADS 原生适配器尚未在本机真实 EDA 环境完成验收；本次仅在 SDK 实际生成及重开回读通过后发布。"}, ValidationStatus: "pending-eda-validation"}
	if req.Config.LibraryMode == "" {
		req.Config.LibraryMode = "new"
	}
	if !req.Config.Enabled {
		return result, errors.New("请先启用本机 EDA 原生集成；PADS ASCII 导出与原生库生成是不同操作")
	}
	if req.Config.LibraryMode != "new" && req.Config.LibraryMode != "append" {
		return result, errors.New("PADS 原生库模式必须为 new 或 append")
	}
	if len(req.Parts) == 0 || len(req.Parts) > 500 || !req.IncludeSymbol && !req.IncludeFootprint {
		return result, errors.New("请选择 1–500 个器件及至少一种导出内容")
	}
	if !filepath.IsAbs(req.OutputDirectory) {
		return result, errors.New("PADS 输出目录必须为绝对路径")
	}
	if err := directPath(req.OutputDirectory); err != nil {
		return result, err
	}
	if req.Config.LibraryMode == "append" {
		for _, path := range []string{req.Config.PADSSymbolLibrary, req.Config.PADSFootprintLibrary} {
			if path != "" {
				base, e := padsNativeBase(path)
				if e != nil {
					return result, e
				}
				for _, ext := range padsLibraryExtensions {
					if e = directPath(base + ext); e != nil {
						return result, e
					}
				}
			}
		}
	}
	if err := ctx.Err(); err != nil {
		return result, err
	}
	sdk, err := deps.inspect(req.Config.PADSRoot, req.IncludeSymbol, req.IncludeFootprint)
	if err != nil {
		return result, err
	}
	if err = os.MkdirAll(req.OutputDirectory, 0755); err != nil {
		return result, err
	}
	stage, err := os.MkdirTemp(req.OutputDirectory, ".packagestudio-pads-native-")
	if err != nil {
		return result, err
	}
	engineAttempted := false
	defer func() {
		if returnErr != nil && engineAttempted {
			if directory, files, e := padsSaveFailure(stage, req.OutputDirectory); e == nil {
				result.Directory = directory
				result.Files = files
				result.ValidationStatus = "native-failed-pending-eda-validation"
				returnErr = fmt.Errorf("%w；诊断目录：%s", returnErr, directory)
			} else {
				returnErr = fmt.Errorf("%w；诊断保存失败：%v", returnErr, e)
			}
		}
		os.RemoveAll(stage)
	}()
	var token [24]byte
	if _, err = rand.Read(token[:]); err != nil {
		return result, err
	}
	nonce := hex.EncodeToString(token[:])
	id := core.SafeName(req.LibraryName) + "_PADS_NATIVE_" + time.Now().Format("20060102_150405") + "_" + nonce[:8]
	destination := filepath.Join(req.OutputDirectory, id)
	backup := filepath.Join(req.OutputDirectory, "PackageStudio_Backups", id)
	parts := append([]core.Part{}, req.Parts...)
	for i := range parts {
		if parts[i].Model != nil {
			model := *parts[i].Model
			parts[i].Model = &model
			if !req.IncludeModel {
				parts[i].Model.Path = ""
			}
		}
	}
	ascii, err := core.ExportPADS(parts, stage, req.LibraryName, req.IncludeSymbol, req.IncludeFootprint)
	if err != nil {
		return result, err
	}
	expected, err := padsExpected(ascii.Directory, req.IncludeSymbol, req.IncludeFootprint)
	if err != nil {
		return result, err
	}
	var pcb, sch, partTypes string
	for _, file := range ascii.Files {
		switch strings.ToLower(filepath.Ext(file)) {
		case ".d":
			pcb = file
		case ".c":
			sch = file
		case ".p":
			partTypes = file
		}
	}
	if partTypes == "" || req.IncludeFootprint && pcb == "" || req.IncludeSymbol && sch == "" {
		return result, errors.New("PADS ASCII 批次缺少必需的输入文件")
	}

	// Native libraries are four-file sets. Each distinct original is copied once,
	// and only its staging base is passed to the SDK helper, never the original.
	jobs := []padsLibraryJob{}
	updates := []FileUpdate{}
	byDestination := map[string]int{}
	add := func(target, role string, items padsItems) error {
		base := ""
		if req.Config.LibraryMode == "append" {
			var e error
			base, e = padsNativeBase(target)
			if e != nil {
				return fmt.Errorf("PADS %s：%w", role, e)
			}
		} else {
			base = filepath.Join(destination, "native", core.SafeName(req.LibraryName))
		}
		key := strings.ToLower(filepath.Clean(base))
		if index, ok := byDestination[key]; ok {
			jobs[index].Expected = mergePADSItems(jobs[index].Expected, items)
			if role == "footprint" {
				jobs[index].PCB = pcb
			} else {
				jobs[index].Schematic = sch
				jobs[index].PartTypes = partTypes
				jobs[index].PartTypeEngine = "logic"
			}
			return nil
		}
		index := len(jobs)
		byDestination[key] = index
		stagedBase := filepath.Join(stage, "native", fmt.Sprintf("library-%d", index+1), "PackageStudio_"+nonce[:12])
		if err := os.MkdirAll(filepath.Dir(stagedBase), 0755); err != nil {
			return err
		}
		job := padsLibraryJob{ID: fmt.Sprintf("library-%d", index+1), Base: stagedBase, Existing: req.Config.LibraryMode == "append", Expected: items, PartTypes: partTypes, PartTypeEngine: "layout"}
		if role == "footprint" {
			job.PCB = pcb
		} else {
			job.Schematic = sch
			job.PartTypeEngine = "logic"
		}
		for _, ext := range padsLibraryExtensions {
			before := ""
			if job.Existing {
				before, err = HashFile(base + ext)
				if err != nil {
					return fmt.Errorf("PADS 追加需要完整四件套，无法读取 %s：%w", base+ext, err)
				}
				if err = padsCopyFile(base+ext, stagedBase+ext); err != nil {
					return err
				}
				copied, e := HashFile(stagedBase + ext)
				if e != nil || copied != before {
					return errors.New("PADS 原库在复制期间发生变化，未启动 SDK")
				}
			}
			updates = append(updates, FileUpdate{StagedPath: stagedBase + ext, Destination: base + ext, BeforeSHA256: before})
		}
		jobs = append(jobs, job)
		return nil
	}
	if req.IncludeFootprint {
		if err = add(req.Config.PADSFootprintLibrary, "footprint", padsItems{Decals: expected.Decals, Parts: expected.Parts}); err != nil {
			return result, err
		}
	}
	if req.IncludeSymbol {
		if err = add(req.Config.PADSSymbolLibrary, "symbol", padsItems{Symbols: expected.Symbols, Parts: expected.Parts}); err != nil {
			return result, err
		}
	}
	// When both roles share a library, its Part Types are imported by Logic only.
	job := padsJob{Protocol: 1, Nonce: nonce, Phase: "convert", Root: stage, SDK: sdk, Libraries: jobs}
	ctx, cancel := context.WithTimeout(ctx, 3*time.Minute)
	defer cancel()
	engineAttempted = true
	converted, err := deps.run(ctx, job)
	if err != nil {
		return result, fmt.Errorf("PADS SDK 生成失败，正式库未发布：%w", err)
	}
	if err = validatePADSEvidence(job, converted, nil); err != nil {
		return result, err
	}
	beforeVerify, err := padsArtifactHashes(jobs)
	if err != nil {
		return result, err
	}
	job.Phase = "verify"
	verified, err := deps.run(ctx, job)
	if err != nil {
		return result, fmt.Errorf("PADS 原生库重开回读失败，正式库未发布：%w", err)
	}
	if err = validatePADSEvidence(job, verified, &converted); err != nil {
		return result, err
	}
	afterVerify, err := padsArtifactHashes(jobs)
	if err != nil {
		return result, err
	}
	for path, hash := range beforeVerify {
		if afterVerify[path] != hash {
			return result, errors.New("PADS 只读回读阶段改变了原生库，已停止发布")
		}
	}
	if err = ctx.Err(); err != nil {
		return result, err
	}
	result.Warnings = append(result.Warnings, ascii.Warnings...)
	result.Warnings = append(result.Warnings, converted.Warnings...)
	result.Warnings = append(result.Warnings, verified.Warnings...)
	result.Warnings = append(result.Warnings, "已由本机 PADS SDK 重开核对库条目名称与数量；引脚、电气映射、图层及实际放置仍需在 PADS 中验收。")
	if req.IncludeModel {
		result.Warnings = append(result.Warnings, "STEP 仅作为独立附件保留；本次原生集成没有建立 Reuse/3DModels 自动关联。")
	}
	// Keep original source/attribution, pin mapping and ASCII rebuilding material.
	for _, file := range ascii.Files {
		rel, e := filepath.Rel(ascii.Directory, file)
		if e != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(os.PathSeparator)) {
			return result, errors.New("PADS ASCII 路径超出批次")
		}
		updates = append(updates, FileUpdate{StagedPath: file, Destination: filepath.Join(destination, "interchange", rel)})
	}
	report := map[string]any{"generator": "Package Studio", "artifactType": "pads-native-library", "nativeGenerated": true, "runtimeVerified": true, "validationStatus": "native-names-counts-readback", "sdk": sdk, "conversion": converted, "verification": verified, "warnings": result.Warnings}
	raw, _ := json.MarshalIndent(report, "", "  ")
	reportPath := filepath.Join(stage, "native-validation.json")
	if err = os.WriteFile(reportPath, raw, 0644); err != nil {
		return result, err
	}
	updates = append(updates, FileUpdate{StagedPath: reportPath, Destination: filepath.Join(destination, "native-validation.json")})
	readmePath := filepath.Join(stage, "README-native.txt")
	if err = os.WriteFile(readmePath, []byte("PADS 原生四件套已经由所选本机 SDK 生成并重新打开核对条目名称和数量。\n请成套保留 .pt9/.pd9/.ld9/.ln9，并在 PADS 中手动登记最终库路径。\n追加模式的正式库位于你选定的原位置；本批次 interchange 仅为重建材料。\n引脚映射、焊盘、制造图层和放置效果仍需实际 EDA 验收。STEP 附件不代表 Reuse 3D 已关联。\n"), 0644); err != nil {
		return result, err
	}
	updates = append(updates, FileUpdate{StagedPath: readmePath, Destination: filepath.Join(destination, "README-native.txt")})
	files, err := deps.publish(updates, backup)
	if err != nil {
		result.BackupDirectory = backup
		return result, fmt.Errorf("PADS 原生库事务发布失败；恢复备份目录 %s：%w", backup, err)
	}
	result.Directory = destination
	result.Files = files
	result.ArtifactType = "pads-native-library"
	result.NativeGenerated = true
	result.RuntimeVerified = true
	result.ValidationStatus = "native-names-counts-readback"
	if req.Config.LibraryMode == "append" {
		result.BackupDirectory = backup
	}
	return result, nil
}

func padsExpected(directory string, symbol, footprint bool) (padsItems, error) {
	raw, err := os.ReadFile(filepath.Join(directory, "conversion-report.json"))
	if err != nil {
		return padsItems{}, err
	}
	var report struct {
		Parts []struct {
			Entry string `json:"entry"`
			Gates []struct {
				Name string `json:"decal"`
			} `json:"gates"`
		} `json:"parts"`
	}
	if err = json.Unmarshal(raw, &report); err != nil {
		return padsItems{}, err
	}
	items := padsItems{}
	for _, p := range report.Parts {
		items.Parts = append(items.Parts, p.Entry)
		if footprint {
			items.Decals = append(items.Decals, p.Entry)
		}
		if symbol {
			for _, gate := range p.Gates {
				items.Symbols = append(items.Symbols, gate.Name)
			}
		}
	}
	if err = validatePADSItems(items); err != nil {
		return padsItems{}, err
	}
	if len(items.Parts) == 0 || len(items.Parts)+len(items.Symbols)+len(items.Decals) > 5000 {
		return padsItems{}, errors.New("PADS 原生批次需为 1–5000 个条目，请缩小批次")
	}
	return items, nil
}

func mergePADSItems(a, b padsItems) padsItems {
	merge := func(x, y []string) []string {
		seen := map[string]bool{}
		z := []string{}
		for _, name := range append(append([]string{}, x...), y...) {
			key := strings.ToUpper(name)
			if !seen[key] {
				seen[key] = true
				z = append(z, name)
			}
		}
		return z
	}
	return padsItems{Parts: merge(a.Parts, b.Parts), Decals: merge(a.Decals, b.Decals), Symbols: merge(a.Symbols, b.Symbols), Drawings: merge(a.Drawings, b.Drawings)}
}

func padsItemGroups(items padsItems) [][]string {
	return [][]string{items.Parts, items.Decals, items.Symbols, items.Drawings}
}
func validatePADSItems(items padsItems) error {
	for _, group := range padsItemGroups(items) {
		seen := map[string]bool{}
		if len(group) > 100000 {
			return errors.New("PADS SDK 库条目数量超出限制")
		}
		for _, name := range group {
			if name == "" || len(name) > 240 || strings.ContainsAny(name, "\r\n\x00|") {
				return errors.New("PADS SDK 返回了无效库条目名称")
			}
			key := strings.ToUpper(name)
			if seen[key] {
				return fmt.Errorf("PADS 同类库条目重名：%s", name)
			}
			seen[key] = true
		}
	}
	return nil
}

func validatePADSEvidence(job padsJob, e padsEvidence, converted *padsEvidence) error {
	bad := func(reason string) error {
		return fmt.Errorf("PADS SDK 证据验证失败，未发布正式库：%s", reason)
	}
	if e.Protocol != 1 || e.Nonce != job.Nonce || e.Phase != job.Phase || !padsSamePath(e.Root, job.Root) || e.Engine != "PADS" || !e.NativeGenerated || len(e.Libraries) != len(job.Libraries) {
		return bad("批次、引擎或产物标识不符")
	}
	if job.Phase == "verify" && !e.RuntimeVerified {
		return bad("缺少独立重开验证")
	}
	seen := map[string]bool{}
	for _, expected := range job.Libraries {
		var actual *padsLibraryEvidence
		for i := range e.Libraries {
			if e.Libraries[i].ID == expected.ID {
				if actual != nil {
					return bad("重复库标识")
				}
				actual = &e.Libraries[i]
			}
		}
		if actual == nil || seen[actual.ID] || !padsSamePath(actual.Base, expected.Base) {
			return bad("库路径或标识不符")
		}
		seen[actual.ID] = true
		if err := validatePADSItems(actual.Before); err != nil {
			return bad(err.Error())
		}
		if err := validatePADSItems(actual.After); err != nil {
			return bad(err.Error())
		}
		if converted != nil {
			if !actual.Reopened {
				return bad("库没有重新打开")
			}
			var prior *padsLibraryEvidence
			for i := range converted.Libraries {
				if converted.Libraries[i].ID == actual.ID {
					prior = &converted.Libraries[i]
				}
			}
			if prior == nil || !padsItemsEqual(actual.After, prior.After) || !padsItemsEqual(actual.Before, prior.After) {
				return bad("重开后条目或数量与生成阶段不一致")
			}
		} else {
			if !expected.Existing && !padsItemsEmpty(actual.Before) {
				return bad("新建库包含未知条目")
			}
			for i, group := range padsItemGroups(expected.Expected) {
				for _, name := range group {
					for _, old := range padsItemGroups(actual.Before)[i] {
						if strings.EqualFold(old, name) {
							return bad("追加冲突，拒绝覆盖同名条目 " + name)
						}
					}
				}
			}
			if !padsItemsEqual(actual.After, mergePADSItems(actual.Before, expected.Expected)) {
				return bad("新增条目缺失或原有条目发生变化")
			}
		}
	}
	return nil
}

func padsItemsEmpty(i padsItems) bool {
	for _, g := range padsItemGroups(i) {
		if len(g) > 0 {
			return false
		}
	}
	return true
}
func padsItemsEqual(a, b padsItems) bool {
	for i, x := range padsItemGroups(a) {
		y := padsItemGroups(b)[i]
		if len(x) != len(y) {
			return false
		}
		xs := append([]string{}, x...)
		ys := append([]string{}, y...)
		for j := range xs {
			xs[j] = strings.ToUpper(xs[j])
			ys[j] = strings.ToUpper(ys[j])
		}
		sort.Strings(xs)
		sort.Strings(ys)
		for j := range xs {
			if xs[j] != ys[j] {
				return false
			}
		}
	}
	return true
}

func padsNativeBase(path string) (string, error) {
	if !filepath.IsAbs(path) || strings.ContainsAny(path, "\x00\r\n") {
		return "", errors.New("请选择现有 PADS .pt9 库的绝对路径")
	}
	if strings.ToLower(filepath.Ext(path)) != ".pt9" {
		return "", errors.New("PADS 库入口必须为 .pt9；四个同基名文件将成套处理")
	}
	return strings.TrimSuffix(filepath.Clean(path), filepath.Ext(path)), nil
}

func padsSamePath(a, b string) bool {
	return filepath.IsAbs(a) && filepath.IsAbs(b) && strings.EqualFold(filepath.Clean(a), filepath.Clean(b))
}
func padsCopyFile(source, destination string) error {
	info, err := os.Lstat(source)
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() || info.Size() == 0 || info.Size() > 256*1024*1024 {
		return errors.New("PADS 库文件必须是 1 字节至 256 MiB 的普通文件")
	}
	in, err := os.Open(source)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.OpenFile(destination, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	_, err = io.Copy(out, io.LimitReader(in, 256*1024*1024+1))
	closeErr := out.Close()
	if err != nil {
		return err
	}
	return closeErr
}

func padsArtifactHashes(libraries []padsLibraryJob) (map[string]string, error) {
	result := map[string]string{}
	for _, lib := range libraries {
		for _, ext := range padsLibraryExtensions {
			path := lib.Base + ext
			if err := directPath(path); err != nil {
				return nil, err
			}
			info, err := os.Lstat(path)
			if err != nil || !info.Mode().IsRegular() || info.Size() == 0 || info.Size() > 256*1024*1024 {
				return nil, fmt.Errorf("PADS SDK 缺少有效原生四件套文件：%s", filepath.Base(path))
			}
			f, err := os.Open(path)
			if err != nil {
				return nil, err
			}
			head := make([]byte, 96)
			n, _ := f.Read(head)
			f.Close()
			if strings.Contains(strings.ToUpper(string(head[:n])), "*PADS-LIBRARY-") {
				return nil, errors.New("PADS SDK 返回的是改后缀的 ASCII 文件，拒绝作为原生库发布")
			}
			h, err := HashFile(path)
			if err != nil {
				return nil, err
			}
			result[path] = h
		}
	}
	return result, nil
}

func inspectPADSSDK(root string, symbol, footprint bool) (padsSDK, error) {
	if runtime.GOOS != "windows" {
		return padsSDK{}, errors.New("PADS 原生集成仅支持 Windows；请使用 PADS ASCII 导出")
	}
	if !filepath.IsAbs(root) {
		return padsSDK{}, errors.New("请选择真实 PADS SDD_HOME 安装目录；当前未检测到可用 PADS SDK")
	}
	root = filepath.Clean(root)
	sdk := padsSDK{Root: root, Factory: filepath.Join(root, "Programs", "PadsCLibInt.dll"), Hashes: map[string]string{}}
	files := []string{sdk.Factory}
	tlb := strings.TrimSuffix(sdk.Factory, filepath.Ext(sdk.Factory)) + ".tlb"
	if _, err := os.Stat(tlb); err == nil {
		files = append(files, tlb)
	}
	if symbol {
		sdk.Logic = filepath.Join(root, "Programs", "powerlogic.exe")
		files = append(files, sdk.Logic, filepath.Join(root, "Programs", "powerlogic.ini"), filepath.Join(root, "Settings", "default.sch"))
	}
	if footprint {
		sdk.Layout = filepath.Join(root, "Programs", "powerpcb.exe")
		files = append(files, sdk.Layout, filepath.Join(root, "Programs", "powerpcb.ini"), filepath.Join(root, "Settings", "default.pcb"))
	}
	for _, path := range files {
		if err := directPath(path); err != nil {
			return padsSDK{}, err
		}
		info, err := os.Lstat(path)
		if err != nil || !info.Mode().IsRegular() || info.Size() == 0 {
			return padsSDK{}, fmt.Errorf("所选安装缺少必需 PADS SDK 文件：%s", path)
		}
		if strings.HasSuffix(strings.ToLower(path), ".exe") || strings.HasSuffix(strings.ToLower(path), ".dll") {
			image, err := pe.Open(path)
			if err != nil {
				return padsSDK{}, fmt.Errorf("无效 PADS SDK PE 文件：%s", path)
			}
			machine := image.Machine
			isDLL := image.Characteristics&pe.IMAGE_FILE_DLL != 0
			image.Close()
			if machine != pe.IMAGE_FILE_MACHINE_I386 || isDLL != strings.HasSuffix(strings.ToLower(path), ".dll") {
				return padsSDK{}, errors.New("当前 PADS 适配器仅支持 Classic 32 位 SDK，不支持 Pro/64 位或伪装文件")
			}
			if !isDLL {
				identity, e := padsFileIdentity(path)
				if e != nil {
					return padsSDK{}, e
				}
				if !containsPADS(identity.Product+" "+identity.Description) || strings.Contains(strings.ToLower(identity.Product+identity.Description), "packagetool") {
					return padsSDK{}, errors.New("所选程序不是具有 PADS 产品元数据的真实安装")
				}
				if !regexp.MustCompile(`(^|[^0-9])9\.4([^0-9]|$)`).MatchString(identity.Version) {
					return padsSDK{}, errors.New("当前原生适配器仅开放文档确认的 PADS Classic 9.4；其他版本需真实 SDK 验收后适配")
				}
			}
		}
		hash, err := HashFile(path)
		if err != nil {
			return padsSDK{}, err
		}
		sdk.Hashes[path] = hash
	}
	return sdk, nil
}

func containsPADS(text string) bool {
	value := strings.ToLower(text)
	return strings.Contains(value, "pads") || strings.Contains(value, "powerpcb") || strings.Contains(value, "powerlogic")
}

// Preserve bounded rebuilding/diagnostic text, never copied SDK executables,
// INI settings, templates or a user's staged native four-file set.
func padsSaveFailure(stage, output string) (string, []string, error) {
	if err := directPath(output); err != nil {
		return "", nil, err
	}
	directory, err := os.MkdirTemp(output, "PADSFailed_")
	if err != nil {
		return "", nil, err
	}
	files := []string{}
	var total int64
	err = filepath.WalkDir(stage, func(path string, entry os.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if entry.IsDir() {
			return nil
		}
		name := entry.Name()
		ext := strings.ToLower(filepath.Ext(name))
		allowed := strings.HasPrefix(name, "request-") && ext == ".json" || strings.HasPrefix(name, "response-") && ext == ".json" || strings.HasPrefix(name, "helper-") && ext == ".log" || name == "studio.bas" || name == "inventory.txt" || name == "conversion-report.json" || name == "layer-mapping.json" || ext == ".d" || ext == ".c" || ext == ".p"
		if !allowed || len(files) >= 64 {
			return nil
		}
		if e := directPath(path); e != nil {
			return e
		}
		info, e := entry.Info()
		if e != nil {
			return e
		}
		if !info.Mode().IsRegular() || info.Size() == 0 || info.Size() > 8*1024*1024 || total+info.Size() > 32*1024*1024 {
			return nil
		}
		rel, e := filepath.Rel(stage, path)
		if e != nil {
			return e
		}
		target := filepath.Join(directory, rel)
		if e = os.MkdirAll(filepath.Dir(target), 0700); e != nil {
			return e
		}
		if e = padsCopyFile(path, target); e != nil {
			return e
		}
		total += info.Size()
		files = append(files, target)
		return nil
	})
	return directory, files, err
}
