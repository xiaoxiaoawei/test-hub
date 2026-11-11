package eda

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"debug/pe"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"packagestudio/internal/core"
)

type cadenceProcessSpec struct {
	Executable                                                     string
	Args                                                           []string
	Directory, ReportPath, ErrorPath, PIDPath, Phase, Entry, Nonce string
}
type cadenceRunner interface {
	Run(context.Context, cadenceProcessSpec) (int, error)
}

// DetectCadence only discovers candidates. It does not start an EDA or validate
// a license, SDK compatibility, or a native conversion.
func DetectCadence() []Installation {
	result := []Installation{}
	for _, candidate := range core.DetectCadenceTools() {
		architecture, _ := cadencePEArchitecture(candidate.Executable)
		sum := sha256.Sum256([]byte(strings.ToLower(candidate.Executable)))
		result = append(result, Installation{ID: "cadence-" + hex.EncodeToString(sum[:5]), Product: candidate.Product, Root: filepath.Dir(candidate.Executable), Executable: candidate.Executable, Version: candidate.VersionHint, Architecture: architecture, Source: candidate.Source, Ready: cadenceCheckEngine(candidate.Executable, candidate.Product) == nil, RuntimeVerified: false, Message: "仅检测到安装候选；须在所选 EDA 中执行生成和重新打开回读，不能据此认定授权或兼容。"})
	}
	return result
}

func ExportCadence(ctx context.Context, request Request) (core.ExportResult, error) {
	return exportCadence(ctx, request, cadenceExecRunner{})
}

func exportCadence(ctx context.Context, request Request, runner cadenceRunner) (result core.ExportResult, err error) {
	result = core.ExportResult{Files: []string{}, Warnings: []string{}, ArtifactType: "native", ValidationStatus: "pending-eda-validation"}
	if err = ctx.Err(); err != nil {
		return result, err
	}
	if !request.Config.Enabled {
		return result, errors.New("请先启用并配置本地 Cadence 原生引擎")
	}
	mode := request.Config.LibraryMode
	if mode == "" {
		mode = "new"
	}
	if mode != "new" && mode != "append" {
		return result, errors.New("Cadence库模式必须为new或append")
	}
	if len(request.Parts) == 0 || !request.IncludeSymbol && !request.IncludeFootprint {
		return result, errors.New("请选择器件及至少一种导出内容")
	}
	if err = directPath(request.OutputDirectory); err != nil {
		return result, err
	}
	for _, part := range request.Parts {
		if err = core.ValidatePart(part, request.IncludeSymbol, request.IncludeFootprint); err != nil {
			return result, fmt.Errorf("%s：%w", part.Name, err)
		}
	}
	captureExe, allegroExe := request.Config.CaptureExe, request.Config.AllegroExe
	for _, candidate := range core.DetectCadenceTools() {
		if cadenceCheckEngine(candidate.Executable, candidate.Product) != nil {
			continue
		}
		if captureExe == "" && candidate.Product == "Capture" {
			captureExe = candidate.Executable
		}
		if allegroExe == "" && candidate.Product == "Allegro" {
			allegroExe = candidate.Executable
		}
	}
	if request.IncludeSymbol {
		if err = cadenceCheckEngine(captureExe, "Capture"); err != nil {
			return result, err
		}
	}
	if request.IncludeFootprint {
		if err = cadenceCheckEngine(allegroExe, "Allegro"); err != nil {
			return result, err
		}
	}
	if mode == "append" {
		if request.IncludeSymbol {
			if !strings.EqualFold(filepath.Ext(request.Config.CaptureLibrary), ".olb") {
				return result, errors.New("请选择要追加的现有Capture .olb库")
			}
			if err = cadenceRegularFile(request.Config.CaptureLibrary); err != nil {
				return result, fmt.Errorf("已有Capture库：%w", err)
			}
		}
		if request.IncludeFootprint {
			if err = directPath(request.Config.AllegroDirectory); err != nil {
				return result, err
			}
			info, e := os.Stat(request.Config.AllegroDirectory)
			if e != nil || !info.IsDir() {
				return result, errors.New("请选择现有Allegro原生库目录")
			}
		}
	}
	if err = os.MkdirAll(request.OutputDirectory, 0755); err != nil {
		return result, err
	}
	stage, err := os.MkdirTemp(request.OutputDirectory, ".cadence-native-stage-")
	if err != nil {
		return result, err
	}
	defer os.RemoveAll(stage)
	engineAttempted := false
	defer func() {
		if err == nil || !engineAttempted {
			return
		}
		failed, saveErr := os.MkdirTemp(request.OutputDirectory, "CadenceFailed_")
		if saveErr == nil {
			saveErr = copyCadenceFailureEvidence(stage, failed)
		}
		if saveErr != nil {
			err = fmt.Errorf("%w；失败诊断保存失败：%v", err, saveErr)
			return
		}
		result.Directory = failed
		if result.ValidationStatus != "publication-failed" {
			result.ValidationStatus = "engine-validation-failed"
		}
		err = fmt.Errorf("%w；诊断脚本与限量日志已保留：%s", err, failed)
	}()
	parts := append([]core.Part(nil), request.Parts...)
	if !request.IncludeModel {
		for i := range parts {
			parts[i].Model = nil
		}
	}
	generated, err := core.ExportCadence(parts, stage, request.LibraryName, request.IncludeSymbol, request.IncludeFootprint)
	if err != nil {
		return result, err
	}
	batch := generated.Directory
	if request.IncludeModel && !request.IncludeFootprint {
		names := core.CadenceEntryNames(parts)
		for i, part := range parts {
			if part.Model == nil || part.Model.Path == "" {
				continue
			}
			ext := strings.ToLower(filepath.Ext(part.Model.Path))
			if ext != ".step" && ext != ".stp" {
				return result, errors.New("Cadence独立模型仅支持STEP/STP")
			}
			if e := cadenceRegularFile(part.Model.Path); e != nil {
				return result, e
			}
			info, statErr := os.Stat(part.Model.Path)
			if statErr != nil {
				return result, statErr
			}
			if info.Size() > 128*1024*1024 {
				return result, errors.New("STEP模型超过128MiB")
			}
			dir := filepath.Join(batch, "step")
			if e := os.MkdirAll(dir, 0700); e != nil {
				return result, e
			}
			if e := copyExclusive(part.Model.Path, filepath.Join(dir, names[i]+ext)); e != nil {
				return result, e
			}
			result.Warnings = append(result.Warnings, "STEP已独立复制；没有自动关联Cadence 3D模型，请在目标软件核对朝向和比例。")
		}
	}
	var entropy [32]byte
	if _, err = rand.Read(entropy[:]); err != nil {
		return result, err
	}
	nonce := hex.EncodeToString(entropy[:])
	plan, err := core.PrepareCadenceNative(batch, parts, nonce, mode == "append" && request.IncludeSymbol, request.Config.CaptureUnitsPerInch)
	if err != nil {
		return result, err
	}
	beforeCapture := ""
	if mode == "append" && request.IncludeSymbol {
		beforeCapture, err = HashFile(request.Config.CaptureLibrary)
		if err != nil {
			return result, err
		}
		staged := filepath.Join(batch, filepath.FromSlash(plan.CaptureLibrary))
		if err = copyExclusive(request.Config.CaptureLibrary, staged); err != nil {
			return result, err
		}
		after, e := HashFile(staged)
		if e != nil || after != beforeCapture {
			return result, errors.New("复制Capture库时原文件发生变化，请关闭EDA后重试")
		}
	}
	// All target-name conflicts are checked before starting any EDA process.
	if mode == "append" && request.IncludeFootprint {
		for _, job := range plan.Allegro {
			for _, rel := range job.ExpectedFiles {
				dst := filepath.Join(request.Config.AllegroDirectory, filepath.Base(rel))
				if err = directPath(dst); err != nil {
					return result, err
				}
				if _, e := os.Lstat(dst); !os.IsNotExist(e) {
					return result, fmt.Errorf("Allegro目标已存在或不可访问，默认拒绝冲突：%s", dst)
				}
			}
		}
	}
	ctx, cancel := context.WithTimeout(ctx, 10*time.Minute)
	defer cancel()
	run := func(spec cadenceProcessSpec) (int, error) {
		engineAttempted = true
		childCtx, stop := context.WithTimeout(ctx, 180*time.Second)
		defer stop()
		spec.Nonce = nonce
		return runner.Run(childCtx, spec)
	}
	if request.IncludeSymbol {
		dir := filepath.Join(batch, "capture")
		for _, phase := range []struct{ script, report, kind string }{{plan.CaptureBuild, plan.CaptureBuildReport, "build"}, {plan.CaptureReadback, plan.CaptureReadbackReport, "readback"}} {
			report := filepath.Join(batch, filepath.FromSlash(phase.report))
			pid, e := run(cadenceProcessSpec{Executable: captureExe, Args: []string{"-i", dir, filepath.Join(batch, filepath.FromSlash(phase.script))}, Directory: dir, ReportPath: report, ErrorPath: filepath.Join(dir, "native-error.txt"), PIDPath: filepath.Join(dir, "engine.pid"), Phase: "capture-" + phase.kind})
			if e != nil {
				return result, fmt.Errorf("Capture %s失败（目标EDA版本尚待验证）：%w", phase.kind, e)
			}
			rows, e := readCadenceReport(report, phase.kind, nonce)
			if e != nil {
				return result, e
			}
			if err = cadenceReportPID(rows, pid); err != nil {
				return result, err
			}
			if phase.kind == "readback" {
				if err = verifyCaptureInventory(rows, parts, plan.Entries, nonce, mode == "append"); err != nil {
					return result, err
				}
			}
		}
		if err = cadenceRegularFile(filepath.Join(batch, filepath.FromSlash(plan.CaptureLibrary))); err != nil {
			return result, fmt.Errorf("Capture未生成有效库：%w", err)
		}
	}
	for i, job := range plan.Allegro {
		dir := filepath.Join(batch, filepath.FromSlash(job.Directory))
		for _, phase := range []struct{ script, report, kind string }{{job.Script, job.BuildReport, "build"}, {job.ReadbackScript, job.ReadbackReport, "readback"}, {job.CompiledScript, job.CompiledReport, "readback-psm"}} {
			args := []string{"-nograph", "-s", filepath.Base(phase.script)}
			if request.Config.AllegroProduct != "" {
				args = append([]string{"-product", request.Config.AllegroProduct}, args...)
			}
			report := filepath.Join(batch, filepath.FromSlash(phase.report))
			_, e := run(cadenceProcessSpec{Executable: allegroExe, Args: args, Directory: dir, ReportPath: report, Phase: "allegro-" + phase.kind, Entry: job.Entry})
			if e != nil {
				return result, fmt.Errorf("Allegro %s失败（目标EDA版本尚待验证）：%w", phase.kind, e)
			}
			rows, e := readCadenceReport(report, phase.kind, nonce)
			if e != nil {
				return result, e
			}
			if phase.kind != "build" {
				if err = verifyAllegroInventory(rows, parts[i], job.Entry); err != nil {
					return result, err
				}
			}
		}
		for _, rel := range job.ExpectedFiles {
			if err = cadenceRegularFile(filepath.Join(batch, filepath.FromSlash(rel))); err != nil {
				return result, fmt.Errorf("Allegro原生产物无效：%w", err)
			}
		}
	}
	if err = ctx.Err(); err != nil {
		return result, err
	}
	artifact, err := os.MkdirTemp(request.OutputDirectory, "CadenceNative_"+time.Now().Format("20060102_150405")+"_")
	if err != nil {
		return result, err
	}
	publicationAttempted := false
	defer func() {
		if !publicationAttempted {
			_ = os.RemoveAll(artifact)
		}
	}()
	evidence, err := copyCadenceEvidence(batch, artifact)
	if err != nil {
		return result, err
	}
	updates := []FileUpdate{}
	seen := map[string]string{}
	add := func(src, dst, before string) error {
		key := strings.ToLower(filepath.Clean(dst))
		if previous, exists := seen[key]; exists {
			a, e := HashFile(previous)
			if e != nil {
				return e
			}
			b, e := HashFile(src)
			if e != nil {
				return e
			}
			if a != b {
				return fmt.Errorf("同名Cadence产物内容不一致：%s", dst)
			}
			return nil
		}
		seen[key] = src
		updates = append(updates, FileUpdate{StagedPath: src, Destination: dst, BeforeSHA256: before})
		return nil
	}
	if request.IncludeSymbol {
		dst := filepath.Join(artifact, "native", filepath.FromSlash(plan.CaptureLibrary))
		if mode == "append" {
			dst = request.Config.CaptureLibrary
		}
		if err = add(filepath.Join(batch, filepath.FromSlash(plan.CaptureLibrary)), dst, beforeCapture); err != nil {
			return result, err
		}
	}
	for _, job := range plan.Allegro {
		for _, rel := range job.ExpectedFiles {
			dst := filepath.Join(artifact, "native", filepath.FromSlash(rel))
			if mode == "append" {
				dst = filepath.Join(request.Config.AllegroDirectory, filepath.Base(rel))
			}
			if err = add(filepath.Join(batch, filepath.FromSlash(rel)), dst, ""); err != nil {
				return result, err
			}
		}
	}
	result.BackupDirectory = filepath.Join(artifact, "backups")
	publicationAttempted = true
	published, err := PublishUpdates(updates, result.BackupDirectory)
	if err != nil {
		result.ValidationStatus = "publication-failed"
		if info, statErr := os.Stat(result.BackupDirectory); statErr == nil && info.IsDir() {
			return result, fmt.Errorf("Cadence发布失败；恢复信息保存在%s：%w", result.BackupDirectory, err)
		}
		result.BackupDirectory = ""
		return result, fmt.Errorf("Cadence发布失败：%w", err)
	}
	result.Directory = artifact
	result.Files = append(published, evidence...)
	result.NativeGenerated = true
	result.RuntimeVerified = true
	result.ValidationStatus = "engine-readback-verified"
	for _, warning := range generated.Warnings {
		if strings.Contains(warning, "当前交付为 Cadence 中间") || strings.Contains(warning, "未发现标准安装") || strings.Contains(warning, "Capture Tcl 已生成，尚未") {
			continue
		}
		result.Warnings = append(result.Warnings, warning)
	}
	result.Warnings = append(result.Warnings, "原生文件已由所选EDA生成，并在独立新进程重新打开回读名称、nonce及引脚映射；这不代替目标版本放置、ERC/DRC和工艺验证。", "本开发机没有Cadence运行环境；该集成的真实目标版本兼容性、Capture比例与原生追加尚待安装EDA后实测。")
	manifestPath := filepath.Join(artifact, "cadence-native-result.json")
	manifest, marshalErr := json.MarshalIndent(result, "", "  ")
	if marshalErr == nil {
		marshalErr = os.WriteFile(manifestPath, manifest, 0600)
	}
	if marshalErr != nil {
		result.Warnings = append(result.Warnings, "原生发布已完成，但结果说明文件写入失败："+marshalErr.Error())
	} else {
		result.Files = append(result.Files, manifestPath)
	}
	return result, nil
}

func cadencePEArchitecture(path string) (string, error) {
	f, err := pe.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	if f.Characteristics&pe.IMAGE_FILE_EXECUTABLE_IMAGE == 0 || f.Characteristics&pe.IMAGE_FILE_DLL != 0 {
		return "", errors.New("不是Windows可执行文件")
	}
	switch f.Machine {
	case pe.IMAGE_FILE_MACHINE_AMD64:
		return "x64", nil
	case pe.IMAGE_FILE_MACHINE_I386:
		return "x86", nil
	default:
		return "", errors.New("不支持的Cadence引擎架构")
	}
}
func cadenceCheckEngine(path, product string) error {
	if path == "" {
		return fmt.Errorf("未安装或未配置%s；无法执行Cadence原生导出，请配置已授权的目标EDA，脚本包仍可单独导出", product)
	}
	name := strings.ToLower(filepath.Base(path))
	allowed := strings.EqualFold(product, "Capture") && name == "capture.exe" || strings.EqualFold(product, "Allegro") && (name == "allegro.exe" || name == "pcb_editor.exe")
	if !allowed {
		return fmt.Errorf("%s引擎文件名不匹配，请选择Capture.exe或Allegro.exe/pcb_editor.exe", product)
	}
	if err := cadenceRegularFile(path); err != nil {
		return fmt.Errorf("%s引擎不可用：%w", product, err)
	}
	if _, err := cadencePEArchitecture(path); err != nil {
		return fmt.Errorf("%s引擎无效：%w", product, err)
	}
	return nil
}
func cadenceRegularFile(path string) error {
	if err := directPath(path); err != nil {
		return err
	}
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() || info.Size() <= 0 || info.Size() > 512*1024*1024 {
		return fmt.Errorf("文件必须是非空且不超过512MiB的普通文件：%s", path)
	}
	return nil
}

func readCadenceReport(path, phase, nonce string) ([][]string, error) {
	if err := cadenceRegularFile(path); err != nil {
		return nil, fmt.Errorf("EDA没有产生回读协议：%w", err)
	}
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	b, err := io.ReadAll(io.LimitReader(f, 4*1024*1024+1))
	if err != nil {
		return nil, err
	}
	if len(b) > 4*1024*1024 {
		return nil, errors.New("EDA回读报告过大")
	}
	rows := [][]string{}
	headers := map[string]string{}
	for _, line := range strings.Split(strings.ReplaceAll(string(b), "\r\n", "\n"), "\n") {
		if line == "" {
			continue
		}
		fields := strings.Split(line, "\t")
		if len(fields) < 2 {
			return nil, errors.New("EDA回读协议损坏")
		}
		switch fields[0] {
		case "protocol", "phase", "nonce", "complete", "pid", "engine", "entry", "preserved":
			if len(fields) != 2 {
				return nil, errors.New("EDA回读头字段损坏")
			}
			if _, exists := headers[fields[0]]; exists {
				return nil, errors.New("EDA回读协议有重复字段")
			}
			headers[fields[0]] = fields[1]
		}
		rows = append(rows, fields)
	}
	if headers["protocol"] != "package-studio.cadence/1" || headers["phase"] != phase || headers["nonce"] != nonce || headers["complete"] != "1" {
		return nil, errors.New("EDA回读阶段、nonce或完成状态不匹配；不会发布")
	}
	if len(rows) == 0 || rows[len(rows)-1][0] != "complete" {
		return nil, errors.New("EDA回读报告没有完整结束")
	}
	if phase != "build" && headers["engine"] == "" {
		return nil, errors.New("EDA回读报告缺少引擎信息")
	}
	return rows, nil
}
func cadenceReportPID(rows [][]string, pid int) error {
	for _, row := range rows {
		if row[0] == "pid" {
			value, err := strconv.Atoi(row[1])
			if err == nil && pid > 0 && value == pid {
				return nil
			}
			return errors.New("Capture回读来自其他进程；拒绝访问已有用户会话")
		}
	}
	return errors.New("Capture缺少独立进程确认")
}
func verifyCaptureInventory(rows [][]string, parts []core.Part, entries []string, nonce string, appendLibrary bool) error {
	actual := []string{}
	packages := map[string]bool{}
	preservedSeen := false
	decode := func(value string) (string, error) { b, e := hex.DecodeString(value); return string(b), e }
	for _, row := range rows {
		switch row[0] {
		case "package":
			if len(row) != 3 {
				return errors.New("Capture包回读字段错误")
			}
			entry, e := decode(row[1])
			if e != nil {
				return e
			}
			value, e := decode(row[2])
			if e != nil {
				return e
			}
			if value != nonce || packages[entry] {
				return errors.New("Capture包nonce或名称重复")
			}
			packages[entry] = true
		case "pin":
			if len(row) != 5 {
				return errors.New("Capture引脚回读字段错误")
			}
			entry, e := decode(row[1])
			if e != nil {
				return e
			}
			unit, e := strconv.Atoi(row[2])
			if e != nil || unit < 1 {
				return errors.New("Capture单元无效")
			}
			number, e := decode(row[3])
			if e != nil {
				return e
			}
			name, e := decode(row[4])
			if e != nil {
				return e
			}
			actual = append(actual, fmt.Sprintf("%q:%d:%q:%q", entry, unit, number, name))
		case "preserved":
			count, e := strconv.Atoi(row[1])
			if e != nil || count < 0 || !appendLibrary && count != 0 {
				return errors.New("Capture已有库回读清单无效")
			}
			preservedSeen = true
		}
	}
	if len(packages) != len(entries) || !preservedSeen {
		return errors.New("Capture包/追加保留清单不完整")
	}
	expected := []string{}
	for i, part := range parts {
		if !packages[entries[i]] {
			return errors.New("Capture回读缺少所选器件")
		}
		for _, pin := range part.Symbol.Pins {
			unit := pin.Unit
			if unit < 1 {
				unit = 1
			}
			expected = append(expected, fmt.Sprintf("%q:%d:%q:%q", entries[i], unit, pin.Number, pin.Name))
		}
	}
	if !sameCadenceStrings(actual, expected) {
		return errors.New("Capture回读的引脚数量、名称、编号或gate与源数据不符")
	}
	return nil
}
func verifyAllegroInventory(rows [][]string, part core.Part, entry string) error {
	actual := []string{}
	entrySeen := false
	for _, row := range rows {
		switch row[0] {
		case "entry":
			name := strings.TrimSuffix(filepath.Base(strings.ReplaceAll(row[1], "\\", "/")), filepath.Ext(row[1]))
			if !strings.EqualFold(name, entry) {
				return errors.New("Allegro回读的封装名称不符")
			}
			entrySeen = true
		case "pin":
			if len(row) != 2 {
				return errors.New("Allegro引脚回读协议损坏")
			}
			actual = append(actual, row[1])
		}
	}
	expected := []string{}
	for _, pin := range part.Footprint.Pads {
		expected = append(expected, pin.Number)
	}
	if !entrySeen || !sameCadenceStrings(actual, expected) {
		return errors.New("Allegro DRA/PSM回读的焊盘数量或编号不符")
	}
	return nil
}
func sameCadenceStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	sort.Strings(a)
	sort.Strings(b)
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
func copyCadenceEvidence(source, target string) ([]string, error) {
	files := []string{}
	err := filepath.WalkDir(source, func(path string, entry os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			return nil
		}
		if entry.Type()&os.ModeSymlink != 0 {
			return errors.New("Cadence stage包含重定向文件")
		}
		ext := strings.ToLower(filepath.Ext(path))
		switch ext {
		case ".json", ".tcl", ".il", ".scr", ".tsv", ".txt", ".ps1", ".step", ".stp", ".log", ".tcldata":
		default:
			return nil
		}
		rel, e := filepath.Rel(source, path)
		if e != nil {
			return e
		}
		dst := filepath.Join(target, "evidence", rel)
		if e = os.MkdirAll(filepath.Dir(dst), 0700); e != nil {
			return e
		}
		if e = copyExclusive(path, dst); e != nil {
			return e
		}
		files = append(files, dst)
		return nil
	})
	return files, err
}

// Preserve at most 32 MiB of diagnostic text, never the copied existing OLB or
// generated native files. The staging tree (including the OLB copy) is removed.
func copyCadenceFailureEvidence(source, target string) error {
	remaining := int64(32 * 1024 * 1024)
	return filepath.WalkDir(source, func(path string, entry os.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if entry.IsDir() || remaining <= 0 {
			return nil
		}
		if entry.Type()&os.ModeSymlink != 0 {
			return errors.New("Cadence诊断包含重定向文件")
		}
		switch strings.ToLower(filepath.Ext(path)) {
		case ".json", ".tcl", ".il", ".scr", ".tsv", ".txt", ".log", ".tcldata":
		default:
			return nil
		}
		rel, e := filepath.Rel(source, path)
		if e != nil {
			return e
		}
		dst := filepath.Join(target, rel)
		if e = os.MkdirAll(filepath.Dir(dst), 0700); e != nil {
			return e
		}
		in, e := os.Open(path)
		if e != nil {
			return e
		}
		defer in.Close()
		out, e := os.OpenFile(dst, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
		if e != nil {
			return e
		}
		limit := min(remaining, int64(4*1024*1024))
		n, copyErr := io.Copy(out, io.LimitReader(in, limit))
		closeErr := out.Close()
		remaining -= n
		if copyErr != nil {
			return copyErr
		}
		return closeErr
	})
}
