package core

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
)

// prepareCadenceModel accepts only the shared, already baked mm STEP contract.
// In particular it never reapplies the source translation/rotation a second time.
func prepareCadenceModel(stage, directory string, part Part, entry string) (ModelAssociation, error) {
	result := ModelAssociation{PartID: part.ID, Entry: entry, Target: "Cadence", Status: "prepared", Message: "已准备 STEP 与 identity 映射脚本；尚未经过目标 EDA 关联和独立回读。"}
	model := part.Model
	if model == nil || !model.Prepared {
		return result, errors.New("Cadence关联需要先准备毫米坐标的STEP模型，不能直接使用未烘焙的源模型")
	}
	if model.PlacementConvention != "step-baked-rhs-mm-v1" {
		return result, errors.New("Cadence已准备模型的坐标约定必须为step-baked-rhs-mm-v1，不能重新应用旧源模型变换")
	}
	ext := strings.ToLower(filepath.Ext(model.Path))
	if ext != ".step" && ext != ".stp" {
		return result, errors.New("Cadence模型关联仅支持STEP/STP")
	}
	in, err := os.Open(model.Path)
	if err != nil {
		return result, err
	}
	defer in.Close()
	info, err := in.Stat()
	if err != nil {
		return result, err
	}
	if !info.Mode().IsRegular() || info.Size() <= 0 || info.Size() > 128*1024*1024 {
		return result, errors.New("STEP模型必须是非空且不超过128MiB的普通文件")
	}
	hash := sha256.New()
	if _, err = io.Copy(hash, in); err != nil {
		return result, err
	}
	sum := hex.EncodeToString(hash.Sum(nil))
	if _, err = in.Seek(0, io.SeekStart); err != nil {
		return result, err
	}
	header := make([]byte, 256)
	n, _ := in.Read(header)
	if !strings.Contains(strings.ToUpper(string(header[:n])), "ISO-10303-21") {
		return result, errors.New("模型不是可识别的STEP交换文件")
	}
	if _, err = in.Seek(0, io.SeekStart); err != nil {
		return result, err
	}
	name := entry + "_" + sum[:16] + ".step"
	rel := directory + "/" + name
	dst := filepath.Join(stage, filepath.FromSlash(rel))
	if err = os.MkdirAll(filepath.Dir(dst), 0700); err != nil {
		return result, err
	}
	out, err := os.OpenFile(dst, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return result, err
	}
	copied := sha256.New()
	_, copyErr := io.Copy(io.MultiWriter(out, copied), in)
	closeErr := out.Close()
	if copyErr != nil {
		return result, copyErr
	}
	if closeErr != nil {
		return result, closeErr
	}
	if hex.EncodeToString(copied.Sum(nil)) != sum {
		return result, errors.New("准备Cadence模型时STEP发生变化")
	}
	result.ModelFile = rel
	result.ModelSHA256 = sum
	result.ModelName = name
	result.ExpectedBBoxMM = append([]float64(nil), model.ExpectedBBoxMM...)
	return result, nil
}

// The Set argument order and six mapping keys are independently observed API
// facts, not a copied application template. Cadence 17.4 SKILL Reference indexes
// axlStepSet p405; public author-tested 17.4 calls corroborate the same order.
// The capability guard deliberately rejects legacy releases without the API.
func cadenceModelMapping(name string) string {
	return fmt.Sprintf(`    unless(isCallable('axlStepSet) error("STEP association requires axlStepSet; this legacy/hotfix is unsupported"))
    axlSetVariable("steppath" ".")
    unless(axlStepSet('primary nil %s '(offset_x 0.0 offset_y 0.0 offset_z 0.0 rotation_x 0.0 rotation_y 0.0 rotation_z 0.0)) error("STEP mapping failed"))
`, skillString(name))
}

// Preparation keeps the model job separate from 2D build.il. An explicit later
// invocation may run it in a NEW batch process, never a user's open session.
func cadencePreparedModelJob(entry, name string) string {
	return fmt.Sprintf(`; Prepared mapping job only. Run in a NEW engine process beside its DRA/STEP.
; No association has been performed or verified merely by generating this file.
procedure(psAssociatePreparedModel()
 let( ()
  unless(axlOpenDesignForBatch(%s "wf") error("Cannot reopen prepared package drawing"))
%s  unless(axlSaveDesign(?design %s ?noConfirm t ?noMru t) error("Mapped drawing save failed"))
  unless(axlCompileSymbol(?symbol %s ?silent t ?silentOnOverwrite t) error("Mapped symbol compile failed"))
  t
 )
)
`, skillString(entry+".dra"), cadenceModelMapping(name), skillString(entry+".dra"), skillString(entry+".psm"))
}

// These persisted properties are documented by the Cadence Community for DRA
// and placed symdefs. We do not guess an undocumented axlStepGet signature.
func cadenceModelReadback(compiled bool, entry string) string {
	object := "design"
	if compiled {
		object = "placed->symdef"
	}
	var b strings.Builder
	fmt.Fprintf(&b, "  modelFile=%s->prop->PKGDEF_STEP_FILE\n  modelTransform=%s->prop->PKGDEF_STEP_TRANSFORMATION\n  unless(stringp(modelFile) && stringp(modelTransform) error(\"Persisted STEP mapping properties are missing\"))\n", object, object)
	b.WriteString("  fprintf(r \"model-file\\t%s\\nmodel-transform\\t%s\\n\" modelFile modelTransform)\n")
	if compiled {
		b.WriteString("  unless(isCallable('axlGetAllAttachmentNames) error(\"Cannot verify compiled STEP facet attachment\"))\n")
		fmt.Fprintf(&b, "  modelAttachment=%s\n  unless(member(modelAttachment axlGetAllAttachmentNames()) error(\"Compiled model has no STEP facet attachment\"))\n  fprintf(r \"model-attachment\\t%%s\\n\" modelAttachment)\n", skillString("STEP3D_"+entry))
	}
	return b.String()
}
