package service

import "packagestudio/internal/eda"

func (s *Service) NativeEnvironment() eda.Environment {
	pads := eda.DetectPADS()
	cadence := eda.DetectCadence()
	if pads == nil {
		pads = []eda.Installation{}
	}
	if cadence == nil {
		cadence = []eda.Installation{}
	}
	return eda.Environment{PADS: pads, Cadence: cadence, Warnings: []string{"环境检测仅确认安装文件；原生生成、已有库追加和 3D 关联仍需在已授权 EDA 环境中实机验证。"}}
}
