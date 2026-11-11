# 版本记录

## 0.3.0

- 迁移到 xiaoxiaoawei/test-hub 的 main 分支，新增 GitHub Actions Windows 云编译、校验与独立版本标签。
- 缩小料号输入区，优化中文字体、辅助文字与紧凑布局。
- 保留立创源 OBJ 材质与 Z 向上坐标，新增顶视和斜视；支持旧版内联 MTL 模型。
- 新增 PADS Logic / Layout ASCII 库导出与多单元引脚映射。
- 新增 Cadence Capture Tcl 与 Allegro SKILL 转换脚本包，需要安装并在目标 EDA 中运行后生成原生库。
- 新增检查更新、下载校验、安装重启、启动确认和失败回滚，保留设置、缓存与导出历史。

## 0.2.0

- 三栏工作台、2D/3D 并排预览、持久化导出历史与快捷键。

## 0.1.0

- Go/Wails 重写，器件获取、2D/3D 预览、KiCad 与可选 Altium 导出。
