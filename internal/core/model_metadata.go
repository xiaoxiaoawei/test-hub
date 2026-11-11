package core

import (
	"errors"
	"strconv"
	"strings"
)

func sourceModelNumber(value any) (float64, error) {
	number, e := strconv.ParseFloat(strings.TrimSpace(str(value)), 64)
	if e != nil || !modelFinite(number) {
		return 0, errors.New("模型元数据不是有限数值")
	}
	return number, nil
}
func sourceModelVector(value any, count int) ([]float64, error) {
	fields := strings.Fields(str(value))
	if strings.Contains(str(value), ",") {
		fields = strings.Split(str(value), ",")
	}
	if len(fields) != count {
		return nil, errors.New("模型元数据向量长度不正确")
	}
	numbers := make([]float64, count)
	for i, field := range fields {
		n, e := sourceModelNumber(field)
		if e != nil {
			return nil, e
		}
		numbers[i] = n
	}
	return numbers, nil
}
func parseSourceModel(attrs object, ox, oy float64) (*Model, string) {
	model := &Model{UUID: str(attrs["uuid"]), ScaleX: 1, ScaleY: 1, ScaleZ: 1, PlacementConvention: "easyeda-std-kicad-v1"}
	bad := false
	origin, e := sourceModelVector(attrs["c_origin"], 2)
	if e != nil {
		bad = true
	} else {
		model.X = (origin[0] - ox) * easyUnit
		model.Y = -(origin[1] - oy) * easyUnit
	}
	rotation, e := sourceModelVector(attrs["c_rotation"], 3)
	if e != nil {
		bad = true
	} else {
		model.RotationX = -rotation[0]
		model.RotationY = -rotation[1]
		model.RotationZ = -rotation[2]
	}
	if value, ok := attrs["z"]; ok && strings.TrimSpace(str(value)) != "" {
		n, e := sourceModelNumber(value)
		if e != nil {
			bad = true
		} else {
			model.Z = n * easyUnit
		}
	}
	for key, destination := range map[string]*float64{"c_width": &model.SourceWidthMM, "c_height": &model.SourceHeightMM} {
		if value, ok := attrs[key]; ok {
			n, e := sourceModelNumber(value)
			if e != nil || n <= 0 {
				bad = true
			} else {
				*destination = n * easyUnit
			}
		}
	}
	if value, ok := attrs["c_scale"]; ok && str(value) != "" {
		scale, e := sourceModelVector(value, 3)
		if e != nil {
			bad = true
		} else {
			if scale[0] <= 0 || scale[1] <= 0 || scale[2] <= 0 {
				bad = true
			}
			model.ScaleX = scale[0]
			model.ScaleY = scale[1]
			model.ScaleZ = scale[2]
		}
	}
	if bad {
		model.PlacementConvention = "unsupported-source-metadata"
		return model, "3D 模型位置/旋转元数据不完整或无效；自动关联前请重新获取并核对，未猜测缺失的对位。"
	}
	return model, ""
}
