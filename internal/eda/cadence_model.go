package eda

import (
	"errors"
	"fmt"
	"math"
	"path/filepath"
	"strconv"
	"strings"

	"packagestudio/internal/core"
)

func cadencePublicationFiles(job core.CadenceAllegroJob) []string {
	files := append([]string(nil), job.ExpectedFiles...)
	if job.ModelFile != "" {
		files = append(files, job.ModelFile)
		if job.ModelSkill != "" {
			files = append(files, job.ModelSkill)
		}
		if job.ModelScript != "" {
			files = append(files, job.ModelScript)
		}
	}
	return files
}

func cadenceVerifyModelSource(batch string, job core.CadenceAllegroJob) error {
	source := filepath.Join(batch, filepath.FromSlash(job.ModelFile))
	if err := cadenceRegularFile(source); err != nil {
		return err
	}
	actual, err := HashFile(source)
	if err != nil {
		return err
	}
	if actual != job.ModelSHA256 || len(actual) != 64 {
		return errors.New("Allegro关联的STEP在引擎运行期间发生变化，不会发布")
	}
	return nil
}

// Persisted properties plus an actual compiled facet attachment are separate
// requirements. Merely returning t from axlStepSet is deliberately insufficient.
func verifyCadenceModelReadback(rows [][]string, job core.CadenceAllegroJob, compiled bool) error {
	values := map[string]string{}
	for _, row := range rows {
		switch row[0] {
		case "model-file", "model-transform", "model-attachment":
			if len(row) != 2 {
				return errors.New("Allegro模型关联回读协议损坏")
			}
			if _, found := values[row[0]]; found {
				return errors.New("Allegro模型关联回读字段重复")
			}
			values[row[0]] = row[1]
		}
	}
	file := strings.TrimSpace(strings.SplitN(values["model-file"], ",", 2)[0])
	if !strings.EqualFold(file, filepath.Base(job.ModelFile)) {
		return fmt.Errorf("Allegro重开后STEP模型文件名不符：%q", file)
	}
	transform := strings.Split(values["model-transform"], ",")
	if len(transform) != 7 {
		return errors.New("Allegro重开后STEP变换缺失或损坏")
	}
	units := strings.ToUpper(strings.TrimSpace(transform[0]))
	if units != "MM" && units != "MILLIMETERS" && units != "MILLIMETRES" {
		return errors.New("Allegro STEP变换未回读为毫米单位")
	}
	for _, component := range transform[1:] {
		value, err := strconv.ParseFloat(strings.TrimSpace(component), 64)
		if err != nil || math.IsNaN(value) || math.IsInf(value, 0) || math.Abs(value) > 1e-8 {
			return errors.New("Allegro重开后的STEP不是已准备的identity变换")
		}
	}
	if compiled && values["model-attachment"] != "STEP3D_"+job.Entry {
		return errors.New("Allegro编译PSM没有对应的STEP facet附件，不能证明引擎实际读取了模型")
	}
	return nil
}
