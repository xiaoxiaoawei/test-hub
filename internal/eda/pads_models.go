package eda

import (
	"bytes"
	"context"
	"crypto/rand"
	"debug/pe"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strings"
	"time"

	"packagestudio/internal/core"
)

const padsModelsWarning = "PADS Reuse 3D 适配尚待真实 VX2.7 / 2019.2 环境验收；脚本准备完成不代表已建立原生关联。"

type padsModelSDK struct {
	Root       string            `json:"root"`
	Executable string            `json:"executable"`
	Version    string            `json:"version"`
	Hashes     map[string]string `json:"hashes"`
}
type padsModelItem struct {
	Existing       bool      `json:"existing"`
	PartID         string    `json:"partId"`
	Entry          string    `json:"entry"`
	Decal          string    `json:"decal"`
	ModelName      string    `json:"modelName"`
	STEP           string    `json:"step"`
	SHA256         string    `json:"sha256"`
	NativeSHA256   string    `json:"nativeSha256,omitempty"`
	ExpectedBBoxMM []float64 `json:"expectedBBoxMm"`
	ToleranceMM    float64   `json:"toleranceMm"`
}
type padsModelJob struct {
	Protocol  int             `json:"protocol"`
	Nonce     string          `json:"nonce"`
	Root      string          `json:"root"`
	Phase     string          `json:"phase"`
	SDK       padsModelSDK    `json:"sdk"`
	Reuse     string          `json:"reuse"`
	PCB       string          `json:"pcb"`
	PartTypes string          `json:"partTypes"`
	Items     []padsModelItem `json:"items"`
}
type padsModelReadback struct {
	PartID        string    `json:"partId"`
	Entry         string    `json:"entry"`
	Decal         string    `json:"decal"`
	ModelName     string    `json:"modelName"`
	Mapped        bool      `json:"mapped"`
	Local         bool      `json:"local"`
	BBoxMM        []float64 `json:"bboxMm"`
	PackageSHA256 string    `json:"packageSha256"`
	LoadedSHA256  string    `json:"loadedSha256"`
	FreshDocument bool      `json:"freshDocument"`
}
type padsModelEvidence struct {
	Protocol int                 `json:"protocol"`
	Nonce    string              `json:"nonce"`
	Root     string              `json:"root"`
	Phase    string              `json:"phase"`
	Reuse    string              `json:"reuse"`
	Version  string              `json:"version"`
	Items    []padsModelReadback `json:"items"`
}
type padsModelDependencies struct {
	inspect func(string) (padsModelSDK, error)
	run     func(context.Context, padsModelJob) (padsModelEvidence, error)
	publish func([]FileUpdate, string) ([]string, error)
}
type padsReuseManifest struct {
	Protocol  int               `json:"protocol"`
	Generator string            `json:"generator"`
	Items     []padsModelItem   `json:"items"`
	Files     map[string]string `json:"files"`
}

// ExportPADSModels keeps 3D association evidence independent of native 2D library
// metadata. Native publication requires a second CAD process and geometry checks.
func ExportPADSModels(ctx context.Context, req Request, result core.ExportResult) (core.ExportResult, error) {
	return exportPADSModels(ctx, req, result, padsModelDependencies{inspectPADSModelSDK, padsRunModelJob, PublishUpdates})
}

func exportPADSModels(ctx context.Context, req Request, existing core.ExportResult, deps padsModelDependencies) (result core.ExportResult, retErr error) {
	result = existing
	result.ModelAssociations = append([]core.ModelAssociation{}, existing.ModelAssociations...)
	firstAssociation := len(result.ModelAssociations)
	if !req.IncludeModel {
		return result, nil
	}
	mode := req.Config.ModelAssociationMode
	if mode == "" {
		mode = "prepare"
	}
	if mode != "prepare" && mode != "native" {
		return result, errors.New("PADS 模型关联模式必须为 prepare 或 native")
	}
	result.Warnings = append(result.Warnings, padsModelsWarning)
	if !req.IncludeFootprint {
		return result, errors.New("PADS 模型关联需要同时导出 PCB 封装")
	}
	if result.NativeGenerated && mode == "native" {
		return result, errors.New("原生 2D 库与现代 Reuse 暂不支持同批提交；请单独使用 ASCII + Reuse 关联，避免跨引擎部分成功")
	}
	if len(req.Parts) == 0 || len(req.Parts) > 50 {
		return result, errors.New("PADS 3D 单批支持 1–50 个器件")
	}
	if err := ctx.Err(); err != nil {
		return result, err
	}
	for _, p := range []string{req.OutputDirectory, result.Directory} {
		if err := directPath(p); err != nil {
			return result, err
		}
	}
	entries, err := padsModelEntries(result.Directory)
	if err != nil {
		return result, err
	}
	stage, err := os.MkdirTemp(req.OutputDirectory, ".packagestudio-pads-models-")
	if err != nil {
		return result, err
	}
	attempted := false
	defer func() {
		if retErr != nil {
			for i := firstAssociation; i < len(result.ModelAssociations); i++ {
				a := &result.ModelAssociations[i]
				if a.Status == "associated" {
					a.Status = "prepared"
					a.Verified = false
					a.GeometryVerified = false
					a.ActualBBoxMM = nil
					a.Message = "候选映射未成功提交；请查看错误和事务恢复目录"
				}
				if a.ModelFile != "" {
					if _, e := os.Stat(a.ModelFile); os.IsNotExist(e) {
						a.ModelFile = ""
					}
				}
			}
		}
		if retErr != nil && attempted {
			if path, _, e := padsSaveFailure(stage, req.OutputDirectory); e == nil {
				retErr = fmt.Errorf("%w；诊断目录：%s", retErr, path)
			}
		}
		os.RemoveAll(stage)
	}()
	var token [24]byte
	if _, err = rand.Read(token[:]); err != nil {
		return result, err
	}
	nonce := hex.EncodeToString(token[:])
	packageName := "PADS_Reuse_" + nonce[:12]
	packageRoot := filepath.Join(stage, "package")
	if err = os.MkdirAll(filepath.Join(packageRoot, "models"), 0700); err != nil {
		return result, err
	}
	job := padsModelJob{Protocol: 1, Nonce: nonce, Root: stage, Phase: "convert", Reuse: filepath.Join(stage, "reuse"), Items: []padsModelItem{}}
	associationIndex := map[string]int{}
	seen := map[string]bool{}
	for _, part := range req.Parts {
		entry, ok := entries[part.ID]
		if !ok || seen[part.ID] {
			return result, fmt.Errorf("PADS 转换清单缺少器件或 ID 重复：%s", part.ID)
		}
		seen[part.ID] = true
		a := core.ModelAssociation{PartID: part.ID, Entry: entry, Target: "PADS Reuse", Status: "unavailable"}
		m := part.Model
		if m == nil || m.Path == "" {
			a.Message = "该器件没有 STEP 模型"
			result.ModelAssociations = append(result.ModelAssociations, a)
			continue
		}
		if !m.Prepared || !padsValidBBox(m.ExpectedBBoxMM) || m.PlacementConvention != "step-baked-rhs-mm-v1" {
			a.Status = "unsupported"
			a.Message = "需要经验证的 mm / 右手 Z 向上 STEP 预处理文件及实际模型边界；未对未校验变换猜测关联"
			result.ModelAssociations = append(result.ModelAssociations, a)
			if mode == "native" {
				return result, fmt.Errorf("%s：%s", part.Name, a.Message)
			}
			continue
		}
		if err = directPath(m.Path); err != nil {
			return result, err
		}
		if ext := strings.ToLower(filepath.Ext(m.Path)); ext != ".step" && ext != ".stp" {
			return result, errors.New("PADS Reuse 只接受 STEP 文件")
		}
		name := "PS_" + entry + "_" + nonce[:8]
		path := filepath.Join(packageRoot, "models", name+".step")
		if err = padsCopyFile(m.Path, path); err != nil {
			return result, err
		}
		hash, e := HashFile(path)
		if e != nil {
			return result, e
		}
		tolerance := m.BBoxToleranceMM
		if tolerance == 0 {
			tolerance = .05
		}
		if math.IsNaN(tolerance) || math.IsInf(tolerance, 0) || tolerance <= 0 || tolerance > .5 {
			return result, errors.New("PADS 几何校验容差须为 0–0.5 mm")
		}
		a.Status = "prepared"
		a.Message = "仅准备关联包；尚未运行 PADS 或登记 Reuse"
		a.ModelName = name + ".step"
		a.ModelFile = filepath.Join(result.Directory, packageName, "models", name+".step")
		a.ModelSHA256 = hash
		a.ExpectedBBoxMM = append([]float64{}, m.ExpectedBBoxMM...)
		associationIndex[part.ID] = len(result.ModelAssociations)
		result.ModelAssociations = append(result.ModelAssociations, a)
		job.Items = append(job.Items, padsModelItem{PartID: part.ID, Entry: entry, Decal: entry, ModelName: name, STEP: path, SHA256: hash, ExpectedBBoxMM: append([]float64{}, m.ExpectedBBoxMM...), ToleranceMM: tolerance})
	}
	if len(job.Items) == 0 {
		return result, nil
	}
	// Independently generated ASCII input and report retain the exact same entry
	// names as the public batch, including duplicate-name suffixes.
	fixtureParts := append([]core.Part{}, req.Parts...)
	for i := range fixtureParts {
		fixtureParts[i].Model = nil
	}
	fixture, e := core.ExportPADS(fixtureParts, stage, "ModelFixture", false, true)
	if e != nil {
		return result, e
	}
	for _, file := range fixture.Files {
		switch strings.ToLower(filepath.Ext(file)) {
		case ".d":
			job.PCB = filepath.Join(stage, "fixture.d")
			err = padsCopyFile(file, job.PCB)
		case ".p":
			job.PartTypes = filepath.Join(stage, "fixture.p")
			err = padsCopyFile(file, job.PartTypes)
		}
		if err != nil {
			return result, err
		}
	}
	if job.PCB == "" || job.PartTypes == "" {
		return result, errors.New("PADS 模型工作工程缺少真实 .d / .p 输入")
	}
	if err = padsCopyFile(job.PCB, filepath.Join(packageRoot, "fixture.d")); err != nil {
		return result, err
	}
	if err = padsCopyFile(job.PartTypes, filepath.Join(packageRoot, "fixture.p")); err != nil {
		return result, err
	}
	portable := append([]padsModelItem{}, job.Items...)
	for i := range portable {
		portable[i].STEP = "models/" + portable[i].ModelName + ".step"
	}
	if err = padsWriteJSON(filepath.Join(packageRoot, "association-plan.json"), map[string]any{"protocol": 1, "mode": "prepared-only", "placementConvention": "step-baked-rhs-mm-v1", "items": portable, "source": "Package Studio generated ASCII; source attribution in parent conversion-report.json", "warning": padsModelsWarning}); err != nil {
		return result, err
	}
	readme := "PADS Reuse model association package\n\nPrepared files are not native associations. Do not rename them to proprietary databases.\nCandidate engine: PADS Classic VX2.7 / 2019.2 x86 only; not PADS Pro or 9.4.\nUse Package Studio native model association with your licensed installation.\nThe adapter never edits global Reuse settings. Manually select the final Reuse in PADS after success.\nA fresh SDK process must read global (not local) mapping, matching XTD hashes, and the expected geometry bounding box in mm.\nExtra native centering, missing licenses, unknown private startup or any mismatch causes failure.\nUnknown existing 3DModels databases cannot safely be merged by this candidate adapter.\nAll actual SDK behavior remains pending real EDA acceptance. See helpers/pads/MODELS.md in the source package.\n"
	if err = os.WriteFile(filepath.Join(packageRoot, "README.txt"), []byte(readme), 0600); err != nil {
		return result, err
	}
	updates := []FileUpdate{}
	backup := ""
	if mode == "native" {
		// Enabled selects the independent 9.4 native 2D engine. Explicit modern
		// model mode and its selected executable/Reuse authorize this operation.
		job.SDK, err = deps.inspect(req.Config.PADSModelExe)
		if err != nil {
			return result, err
		}
		reuse := req.Config.PADSReuseDirectory
		if err = directPath(reuse); err != nil {
			return result, err
		}
		if padsSamePath(reuse, result.Directory) || padsWithin(reuse, result.Directory) || padsWithin(result.Directory, reuse) {
			return result, errors.New("Reuse 目录必须与本批次输出目录分离")
		}
		before, e := padsModelTree(reuse)
		if e != nil {
			return result, e
		}
		managed, e := padsReadReuseManifest(reuse, before)
		if e != nil {
			return result, e
		}
		if err = os.MkdirAll(job.Reuse, 0700); err != nil {
			return result, err
		}
		for rel := range before {
			dest := filepath.Join(job.Reuse, rel)
			if err = os.MkdirAll(filepath.Dir(dest), 0700); err == nil {
				err = padsCopyReuseFile(filepath.Join(reuse, rel), dest)
			}
			if err != nil {
				return result, err
			}
		}
		if len(managed.Items) > 0 {
			if len(managed.Items)+len(job.Items) > 500 {
				return result, errors.New("受管 Reuse 最多支持 500 个映射")
			}
			for _, old := range managed.Items {
				for _, next := range job.Items {
					if old.PartID == next.PartID || strings.EqualFold(old.Entry, next.Entry) && strings.EqualFold(old.Decal, next.Decal) {
						return result, fmt.Errorf("Reuse 已存在同名映射 %s / %s，默认拒绝替换", old.Entry, old.Decal)
					}
				}
			}
			for _, pair := range [][2]string{{"fixture.d", job.PCB}, {"fixture.p", job.PartTypes}} {
				if err = padsMergeModelASCII(filepath.Join(job.Reuse, ".package-studio", pair[0]), pair[1]); err != nil {
					return result, err
				}
			}
			old := append([]padsModelItem{}, managed.Items...)
			for i := range old {
				old[i].Existing = true
				old[i].STEP = filepath.Join(job.Reuse, filepath.FromSlash(old[i].STEP))
			}
			job.Items = append(old, job.Items...)
		}
		attempted = true
		bounded, cancel := context.WithTimeout(ctx, 10*time.Minute)
		defer cancel()
		converted, e := deps.run(bounded, job)
		if e != nil {
			return result, e
		}
		if err = padsValidateModels(job, converted, false); err != nil {
			return result, err
		}
		candidate, e := padsModelTree(job.Reuse)
		if e != nil {
			return result, e
		}
		job.Phase = "verify"
		verified, e := deps.run(bounded, job)
		if e != nil {
			return result, e
		}
		if err = padsValidateModels(job, verified, true); err != nil {
			return result, err
		}
		afterVerify, e := padsModelTree(job.Reuse)
		if e != nil {
			return result, e
		}
		if !padsHashesEqual(candidate, afterVerify) {
			return result, errors.New("PADS 回读修改了候选 Reuse，未提交")
		}
		current, e := padsModelTree(reuse)
		if e != nil {
			return result, e
		}
		if !padsHashesEqual(before, current) {
			return result, errors.New("目标 Reuse 在校验期间发生变化，未提交")
		}
		for rel, hash := range before {
			if candidate[rel] == "" {
				return result, errors.New("SDK 删除了原 Reuse 文件，未提交")
			}
			if !strings.HasPrefix(strings.ToLower(filepath.ToSlash(rel)), "3dmodels/") && candidate[rel] != hash {
				return result, errors.New("SDK 修改或删除了原 Reuse 无关文件，未提交")
			}
		}
		for _, item := range job.Items {
			full := item.ModelName + ".step"
			rel := filepath.Join("3DModels", "PartDB", "Packages", "User", full, "SAT", full+"_1.xtd")
			if candidate[rel] == "" {
				return result, errors.New("PADS 未生成对应的真实 Reuse XTD 模型")
			}
			if st, e := os.Stat(filepath.Join(job.Reuse, rel)); e != nil || st.Size() == 0 {
				return result, errors.New("PADS 生成的原生 XTD 模型为空")
			}
			if item.Existing && candidate[rel] != before[rel] {
				return result, errors.New("旧映射的原生 XTD 在追加期间变化，未提交")
			}
			for _, v := range verified.Items {
				if v.PartID == item.PartID && candidate[rel] != v.PackageSHA256 {
					return result, errors.New("PADS XTD 实际文件与回读报告哈希不符")
				}
			}
		}
		for rel, hash := range candidate {
			if hash != before[rel] && !strings.HasPrefix(strings.ToLower(filepath.ToSlash(rel)), "3dmodels/") {
				return result, errors.New("SDK 在 3DModels 之外写入未知 Reuse 文件")
			}
			if hash != before[rel] && !padsModelDataExtension(rel) {
				return result, errors.New("SDK 生成了未适配的 Reuse 文件类型，未提交")
			}
		}
		metadata := filepath.Join(job.Reuse, ".package-studio")
		if err = os.MkdirAll(filepath.Join(metadata, "models"), 0700); err != nil {
			return result, err
		}
		for _, pair := range [][2]string{{job.PCB, "fixture.d"}, {job.PartTypes, "fixture.p"}} {
			target := filepath.Join(metadata, pair[1])
			if err = padsReplaceCopy(pair[0], target); err != nil {
				return result, err
			}
		}
		manifest := padsReuseManifest{Protocol: 1, Generator: "Package Studio PADS Reuse", Items: append([]padsModelItem{}, job.Items...)}
		for i := range manifest.Items {
			item := &manifest.Items[i]
			rel := filepath.Join(".package-studio", "models", item.ModelName+".step")
			if !item.Existing {
				if err = padsCopyFile(item.STEP, filepath.Join(job.Reuse, rel)); err != nil {
					return result, err
				}
			}
			item.STEP = filepath.ToSlash(rel)
			full := item.ModelName + ".step"
			item.NativeSHA256 = candidate[filepath.Join("3DModels", "PartDB", "Packages", "User", full, "SAT", full+"_1.xtd")]
			item.Existing = false
		}
		manifest.Files, err = padsModelTree(job.Reuse)
		if err != nil {
			return result, err
		}
		delete(manifest.Files, "package-studio-reuse.json")
		if err = padsWriteJSON(filepath.Join(job.Reuse, "package-studio-reuse.json"), manifest); err != nil {
			return result, err
		}
		candidate, err = padsModelTree(job.Reuse)
		if err != nil {
			return result, err
		}
		for rel, hash := range candidate {
			if hash != before[rel] {
				updates = append(updates, FileUpdate{filepath.Join(job.Reuse, rel), filepath.Join(reuse, rel), before[rel]})
			}
		}
		for _, v := range verified.Items {
			i, ok := associationIndex[v.PartID]
			if !ok {
				continue
			}
			result.ModelAssociations[i].Status = "associated"
			result.ModelAssociations[i].Verified = true
			result.ModelAssociations[i].GeometryVerified = true
			result.ModelAssociations[i].ActualBBoxMM = append([]float64{}, v.BBoxMM...)
			result.ModelAssociations[i].Message = "独立 PADS 进程已回读全部旧及新全局映射、模型哈希与几何边界；目标版本仍待 EDA 实机验收"
		}
		if err = padsWriteJSON(filepath.Join(packageRoot, "readback.json"), verified); err != nil {
			return result, err
		}
		backup = filepath.Join(req.OutputDirectory, "PackageStudio_Backups", packageName)
	}
	if err = padsWriteJSON(filepath.Join(packageRoot, "associations.json"), result.ModelAssociations); err != nil {
		return result, err
	}
	files, e := padsModelTree(packageRoot)
	if e != nil {
		return result, e
	}
	for rel := range files {
		updates = append(updates, FileUpdate{filepath.Join(packageRoot, rel), filepath.Join(result.Directory, packageName, rel), ""})
	}
	if backup == "" {
		backup = filepath.Join(req.OutputDirectory, "PackageStudio_Backups", packageName)
	}
	if err = ctx.Err(); err != nil {
		return result, err
	}
	_, err = deps.publish(updates, backup)
	result.BackupDirectory = backup
	if err != nil {
		return result, fmt.Errorf("PADS 3D 提交失败：%w；备份/恢复目录：%s", err, backup)
	}
	for rel := range files {
		result.Files = append(result.Files, filepath.Join(result.Directory, packageName, rel))
	}
	if mode == "native" {
		for _, u := range updates {
			if !padsWithin(u.Destination, result.Directory) {
				result.Files = append(result.Files, u.Destination)
			}
		}
	}
	return result, nil
}

func padsModelEntries(directory string) (map[string]string, error) {
	path := filepath.Join(directory, "conversion-report.json")
	if err := directPath(path); err != nil {
		return nil, err
	}
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	var report struct {
		Parts []struct {
			ID    string `json:"id"`
			Entry string `json:"entry"`
		} `json:"parts"`
	}
	data, err := io.ReadAll(io.LimitReader(f, 16*1024*1024+1))
	if err != nil || len(data) > 16*1024*1024 {
		return nil, errors.New("PADS 转换清单过大或读取失败")
	}
	if err = json.Unmarshal(data, &report); err != nil {
		return nil, err
	}
	result := map[string]string{}
	valid := regexp.MustCompile(`^[A-Z0-9_.$-]{1,40}$`)
	for _, p := range report.Parts {
		if p.ID == "" || !valid.MatchString(p.Entry) || result[p.ID] != "" {
			return nil, errors.New("PADS 转换清单的条目名或器件 ID 无效")
		}
		result[p.ID] = p.Entry
	}
	return result, nil
}
func padsValidBBox(b []float64) bool {
	if len(b) != 6 {
		return false
	}
	for _, v := range b {
		if math.IsNaN(v) || math.IsInf(v, 0) || math.Abs(v) > 100000 {
			return false
		}
	}
	return b[0] < b[3] && b[1] < b[4] && b[2] < b[5]
}
func padsValidateModels(j padsModelJob, e padsModelEvidence, verify bool) error {
	if e.Protocol != 1 || e.Nonce != j.Nonce || e.Phase != j.Phase || !padsSamePath(e.Root, j.Root) || !padsSamePath(e.Reuse, j.Reuse) || !regexp.MustCompile(`^2019\.2\.`).MatchString(e.Version) || len(e.Items) != len(j.Items) {
		return errors.New("PADS 模型验证报告协议、版本或条数不符")
	}
	seen := map[string]bool{}
	for _, item := range j.Items {
		found := false
		for _, v := range e.Items {
			if v.PartID != item.PartID {
				continue
			}
			if seen[v.PartID] || v.Entry != item.Entry || v.Decal != item.Decal || v.ModelName != item.ModelName+".step" || !v.Mapped || !v.FreshDocument || verify && v.Local || !padsValidBBox(v.BBoxMM) {
				return errors.New("PADS 未回读到期望的独立全局模型映射")
			}
			seen[v.PartID] = true
			found = true
			for i, x := range v.BBoxMM {
				if math.Abs(x-item.ExpectedBBoxMM[i]) > item.ToleranceMM {
					return fmt.Errorf("%s：PADS 模型边界与预处理 STEP 不符（可能存在额外自动居中/偏移）", item.Entry)
				}
			}
			if verify && (!regexp.MustCompile(`^[a-f0-9]{64}$`).MatchString(v.PackageSHA256) || v.PackageSHA256 != v.LoadedSHA256) {
				return errors.New("PADS 持久模型与 fresh-board 加载模型哈希不符")
			}
		}
		if !found {
			return errors.New("PADS 模型回读缺少器件")
		}
	}
	return nil
}
func padsModelTree(root string) (map[string]string, error) {
	if err := directPath(root); err != nil {
		return nil, err
	}
	out := map[string]string{}
	var total int64
	nodes := 0
	err := filepath.WalkDir(root, func(path string, d os.DirEntry, e error) error {
		if os.IsNotExist(e) && path == root {
			return nil
		}
		if e != nil {
			return e
		}
		nodes++
		if nodes > 8192 {
			return errors.New("Reuse 目录项超出安全限制")
		}
		if err := directPath(path); err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		st, e := d.Info()
		if e != nil {
			return e
		}
		if !st.Mode().IsRegular() {
			return errors.New("Reuse 只能包含普通文件")
		}
		total += st.Size()
		if len(out) >= 4096 || st.Size() > 256*1024*1024 || total > 512*1024*1024 {
			return errors.New("Reuse 超出安全副本大小限制")
		}
		rel, e := filepath.Rel(root, path)
		if e != nil {
			return e
		}
		hash, e := HashFile(path)
		if e != nil {
			return e
		}
		out[rel] = hash
		return nil
	})
	return out, err
}
func padsHashesEqual(a, b map[string]string) bool {
	if len(a) != len(b) {
		return false
	}
	for k, v := range a {
		if b[k] != v {
			return false
		}
	}
	return true
}
func padsWithin(path, root string) bool {
	rel, e := filepath.Rel(root, path)
	return e == nil && rel != "." && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}
func padsWriteJSON(path string, value any) error {
	data, e := json.MarshalIndent(value, "", "  ")
	if e != nil {
		return e
	}
	return os.WriteFile(path, data, 0600)
}

func inspectPADSModelSDK(executable string) (padsModelSDK, error) {
	sdk := padsModelSDK{Executable: executable, Hashes: map[string]string{}}
	if runtime.GOOS != "windows" {
		return sdk, errors.New("PADS Reuse 原生关联只支持 Windows")
	}
	if err := directPath(executable); err != nil {
		return sdk, err
	}
	if !strings.EqualFold(filepath.Base(executable), "powerpcb.exe") {
		return sdk, errors.New("请选择现代 PADS Classic powerpcb.exe")
	}
	id, err := padsFileIdentity(executable)
	if err != nil {
		return sdk, err
	}
	if !regexp.MustCompile(`(?i)PADS|PowerPCB`).MatchString(id.Product+" "+id.Description) || !regexp.MustCompile(`^2019\.2\.`).MatchString(id.Version) {
		return sdk, errors.New("3D 候选适配仅接受 PADS Classic VX2.7 / 2019.2；9.4、Pro 与未知版本未适配")
	}
	p, err := pe.Open(executable)
	if err != nil {
		return sdk, err
	}
	defer p.Close()
	if p.Machine != pe.IMAGE_FILE_MACHINE_I386 {
		return sdk, errors.New("PADS 3D 候选适配仅支持 x86")
	}
	sdk.Root = filepath.Dir(filepath.Dir(executable))
	sdk.Version = id.Version
	for _, path := range []string{executable, filepath.Join(filepath.Dir(executable), "powerpcb.ini"), filepath.Join(sdk.Root, "Settings", "default.pcb")} {
		if err = directPath(path); err != nil {
			return sdk, err
		}
		hash, e := HashFile(path)
		if e != nil {
			return sdk, e
		}
		sdk.Hashes[path] = hash
	}
	return sdk, nil
}

// Stable ordering makes mock protocol evidence and archive inventories readable.
func padsModelKeys(m map[string]string) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func padsReadReuseManifest(root string, tree map[string]string) (padsReuseManifest, error) {
	var m padsReuseManifest
	path := filepath.Join(root, "package-studio-reuse.json")
	if tree["package-studio-reuse.json"] == "" {
		for rel := range tree {
			first := strings.ToLower(strings.Split(filepath.ToSlash(rel), "/")[0])
			if first == "3dmodels" || first == ".package-studio" {
				return m, errors.New("目标含未知 Reuse 数据库；仅支持空白库或本工具有完整清单的受管库")
			}
		}
		return m, nil
	}
	f, e := os.Open(path)
	if e != nil {
		return m, e
	}
	defer f.Close()
	raw, e := io.ReadAll(io.LimitReader(f, 16*1024*1024+1))
	if e != nil || len(raw) > 16*1024*1024 {
		return m, errors.New("Reuse 管理清单过大")
	}
	if e = json.Unmarshal(raw, &m); e != nil {
		return m, e
	}
	if m.Protocol != 1 || m.Generator != "Package Studio PADS Reuse" || len(m.Items) == 0 || len(m.Items) > 500 {
		return m, errors.New("Reuse 管理清单不受支持")
	}
	actual := map[string]string{}
	for k, v := range tree {
		if k != "package-studio-reuse.json" {
			actual[k] = v
		}
	}
	if !padsHashesEqual(actual, m.Files) {
		return m, errors.New("受管 Reuse 文件已被外部修改；未追加，原库保持不变")
	}
	seen := map[string]bool{}
	names := regexp.MustCompile(`^[A-Za-z0-9_.$-]{1,80}$`)
	ids := map[string]bool{}
	models := map[string]bool{}
	for _, x := range m.Items {
		key := strings.ToLower(x.Entry + "|" + x.Decal)
		rel := filepath.Join(".package-studio", "models", x.ModelName+".step")
		full := x.ModelName + ".step"
		nativeRel := filepath.Join("3DModels", "PartDB", "Packages", "User", full, "SAT", full+"_1.xtd")
		if x.PartID == "" || seen[key] || ids[x.PartID] || models[strings.ToLower(x.ModelName)] || !names.MatchString(x.Entry) || !names.MatchString(x.Decal) || !names.MatchString(x.ModelName) || !padsValidBBox(x.ExpectedBBoxMM) || math.IsNaN(x.ToleranceMM) || math.IsInf(x.ToleranceMM, 0) || x.ToleranceMM <= 0 || x.ToleranceMM > .5 || filepath.FromSlash(x.STEP) != rel || m.Files[rel] != x.SHA256 || x.NativeSHA256 == "" || m.Files[nativeRel] != x.NativeSHA256 {
			return m, errors.New("受管 Reuse 条目/模型来源哈希无效")
		}
		seen[key] = true
		ids[x.PartID] = true
		models[strings.ToLower(x.ModelName)] = true
	}
	return m, nil
}
func padsMergeModelASCII(previous, current string) error {
	old, e := os.ReadFile(previous)
	if e != nil {
		return e
	}
	next, e := os.ReadFile(current)
	if e != nil {
		return e
	}
	old = bytes.TrimSpace(old)
	next = bytes.TrimSpace(next)
	oldEnd := []byte("*END*")
	firstOld := bytes.IndexByte(old, '\n')
	firstNext := bytes.IndexByte(next, '\n')
	if firstOld < 0 || firstNext < 0 || !bytes.Equal(bytes.TrimSpace(old[:firstOld]), bytes.TrimSpace(next[:firstNext])) || !bytes.HasSuffix(old, oldEnd) || !bytes.HasSuffix(next, oldEnd) {
		return errors.New("受管 Reuse fixture ASCII 头或结束标记无效")
	}
	// A library importer stops at END. Remove only the previous terminal marker;
	// retaining it would silently hide all newly appended entries from the SDK.
	merged := append(append(bytes.TrimSpace(old[:len(old)-len(oldEnd)]), '\n'), next[firstNext+1:]...)
	return os.WriteFile(current, append(merged, '\n'), 0600)
}
func padsReplaceCopy(source, target string) error {
	if e := os.Remove(target); e != nil && !os.IsNotExist(e) {
		return e
	}
	return padsCopyFile(source, target)
}
func padsModelDataExtension(path string) bool {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".dat", ".xtd", ".xml", ".txt", ".png", ".bmp", ".jpg", ".step", ".stp", ".sat":
		return true
	}
	return false
}

// Unrelated user files may legitimately be empty. Reuse snapshots therefore
// use a bounded data copier rather than the nonempty native-library copier.
func padsCopyReuseFile(source, target string) error {
	if err := directPath(source); err != nil {
		return err
	}
	in, e := os.Open(source)
	if e != nil {
		return e
	}
	defer in.Close()
	st, e := in.Stat()
	if e != nil {
		return e
	}
	if !st.Mode().IsRegular() || st.Size() > 256*1024*1024 {
		return errors.New("Reuse 文件大小或类型无效")
	}
	out, e := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if e != nil {
		return e
	}
	n, e := io.Copy(out, io.LimitReader(in, 256*1024*1024+1))
	closeErr := out.Close()
	if e != nil {
		return e
	}
	if n > 256*1024*1024 {
		return errors.New("Reuse 文件在复制期间超出限制")
	}
	return closeErr
}
