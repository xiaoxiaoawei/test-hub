# 封装工坊 · Package Studio

以 Go + Wails 构建的 Windows 器件封装工作台。界面为独立编写的 Vue 3 / TypeScript，使用系统 WebView2；不附带 Python、Node.js 或整套浏览器运行环境。

## 使用

运行发行包中的 `PackageStudio.exe`。Windows 10/11 x64，需要 Microsoft WebView2 Runtime。多数 Windows 电脑已有；缺失时可从微软官方安装：
https://developer.microsoft.com/microsoft-edge/webview2/

1. 输入立创 C 编号，例如 `C25804`（两引脚电阻）或 `C23922`（多单元器件），按 Enter 添加。可以一次输入多个编号。
2. 检查封装与原理图预览；滚轮缩放、拖动平移、双击适应视图。三维模型在 3D 页按需加载。
3. 勾选器件、导出内容和目标软件，选择输出目录，开始导出。
4. 到目标 EDA 软件中回读新库，并核对实际尺寸、引脚、图层和模型定位。

快捷键：Ctrl+K 聚焦搜索；搜索框中 Enter 获取、Ctrl+Enter 换行；Ctrl+Shift+Enter 导出，Esc 关闭弹窗。输入法组合输入期间不会触发全局导出。

## 0.3 更新

项目主分支为 [xiaoxiaoawei/test-hub main](https://github.com/xiaoxiaoawei/test-hub/tree/main)。GitHub Actions 自动运行测试、构建 Windows x64 便携包并校验 Windows 内嵌版本、SHA-256、ZIP 结构和导出流程。发布标签采用 `package-studio-v0.3.0`，与仓库其他软件版本分开。

料号输入框默认缩为一行，输入多条时展开。字体采用系统中文 UI 字体，放大辅助文字。3D 保留立创源 OBJ 的颜色、材质和 Z 向上坐标，提供顶视／斜视。预览效果仍取决于源模型质量；不凭空补造丝印或立创标识。

顶栏“检查更新”读取独立稳定通道 `updates/package-studio/stable.json`，发现新版后可下载、验证并安装重启。安装前重新核对 SHA-256、版本及 Windows GUI x64 文件；独立辅助进程等待旧程序退出、替换程序文件，再等待新版启动确认，失败则回滚。设置、器件缓存、历史与已导出库保留。v0.2 没有内置更新入口，需要先手动解压本次 v0.3 包。

## 工作台

参考原软件随附的工作台截图，改为紧凑顶栏、三栏工作区：左侧封装列表及常驻导出历史，中间预览和任务状态，右侧按步骤分组的导出设置。

预览内容和显示布局分别选择，支持原理图／PCB 的 2D 预览与真实 3D 模型并排显示；模型仍然按需加载。导出历史由本地 Go 服务记录真实成功与失败结果，重启后保留，最多保存最近 100 个批次。清除历史不会删除已生成的库。文件入口会在资源管理器中定位实际生成的文件。

“导出后打开目录”和预览布局等选项随设置保存。旧版配置会保留路径、主题等原有选项并补齐新版默认值。

## 当前功能范围

| 功能 | 状态 |
| --- | --- |
| C 编号获取、EasyEDA Standard JSON 导入 | 已实现；取决于数据源可用性 |
| 2D 封装和多单元原理图预览 | 已实现 |
| OBJ 三维预览、按需获取源 STEP | 保留源模型材质；方向与高度需目标 EDA 回读核对 |
| KiCad `.kicad_sym` / `.pretty` / `.kicad_mod` | 已实现；独立批次输出 |
| Altium `.SchLib` / `.PcbLib` | 通过可选 Altium 辅助进程；见下方说明 |
| PADS Logic / Layout | 输出 `.c/.d/.p` ASCII 导入库，保留多单元映射；未在本机 PADS 回读 |
| Cadence Capture / Allegro | 输出 Tcl / SKILL 脚本包；在已安装且已授权的 EDA 中运行，检查完成标记与生成文件；未在本机 Cadence 回读 |
| 检查更新与安装重启 | 独立更新通道，哈希校验、启动确认和失败回滚 |
| 现有库合并、自动注册 EDA 全局库 | 尚未实现 |
| EasyEDA 专业版工程、整板转换 | 当前不支持 |

当前版本是可运行的重写首版，并未宣称与原软件所有行为及 EDA 版本兼容性完全一致。圆弧/曲线可能离散为线段；未支持图元会列入转换提示和报告。内置演示器件为独立绘制，用于操作体验和格式测试。

PADS 不输出伪装成原生二进制库的文件；导出目录内的使用说明描述 Logic 与 Layout 的导入顺序。Cadence 目录包含 `RUN-CADENCE.ps1`、来源数据、脚本与清单。选择本机 EDA 可执行程序运行后，只有预期原生文件及完成标记全部存在才记录转换完成。Capture 脚本对运行时 API 枚举进行检查；数据库单位兼容设定需要在目标版本中核对，可用参数覆盖。具体兼容边界见每批次说明。

KiCad 导出生成新的批次目录，包含库、参考库表、`conversion-report.json` 和使用说明。已有库不会被合并。带模型的库使用当前导出目录的绝对路径；移动目录后需要更新模型关联。报告保留数据来源及每个器件的转换提示。

## Altium 可选适配器

发行包中若有 `adapters/altium/PackageStudio.Altium.dll`，且电脑安装 .NET 10 Runtime，则启用 Altium 导出。辅助进程为新编写的代码，使用公开的 OriginalCircuit.AltiumSharp 1.0.2 库，不依赖原软件 EXE。请保留适配器文件夹。

该路线可以生成独立库文件；当前未在所有 AD 版本中验证。具体支持形状与限制见 `helpers/altium/README.md`。完整 .NET 运行环境不打入主程序，可控制主程序与基础发行包体积。

## 本地数据与性能设计

设置、器件缓存与导出历史保存在 `%APPDATA%/PackageStudio/`。联网用于读取所选器件及模型；几何解析、库生成在本地执行。当前使用 JSON 保存小规模设置、队列与历史，数据库接口可在后续数据量增加时扩展。

后台 Go 服务处理网络、解析与导出。Three.js 按需加载，三维场景静止时停止连续渲染，换模型后释放几何与材质；下载的 OBJ / STEP 放入本地缓存。客户端未内嵌固定版本 WebView2。

## 开发与构建

云构建固定 Go 1.27.4、Node.js 24、Python 3.12 和 .NET 10 SDK。桌面运行需要 Windows WebView2。本机构建使用相同工具版本可减少差异。

```powershell
.\build.ps1
# 或仅构建基础程序：
.\build.ps1 -SkipAltium
# 包含可选 AD 适配器并生成发行 ZIP：
.\build.ps1 -Package
```

输出在 `build/PackageStudio/`。Go 工具链不在 PATH 时可传 `-GoExe 'C:\tools\go\bin\go.exe'`。

测试与调试：

```powershell
go test ./internal/...
# 先构建前端，再启动仅绑定回环地址的调试服务：
go run -tags desktop,production . --serve 127.0.0.1:34116 --data-dir .\test-data
# 另一个终端：
cd frontend
npm run dev
```

调试 HTTP 模式用于开发验证，正式桌面版使用 Wails 进程内绑定。

## 版本发布

1. 同步修改 `VERSION`、`internal/service/service.go`、前端 package 与 lock、`wails.json`、`winres/winres.json` 和 `CHANGELOG.md`。
2. 在 main 提交并等待 `Package Studio Windows` 的 Linux 核心测试和 Windows 构建通过。
3. 创建与 `VERSION` 一致的附注标签，例如 `git tag -a package-studio-v0.3.1 -m 'Package Studio 0.3.1'`，再推送该标签。
4. Actions 再次校验、编译并发布 ZIP、`update.json`、`SHA256SUMS.txt`。下载校验通过后才提交独立 stable feed；同版本资产禁止覆盖，feed 禁止回退。

发行页使用 `--latest=false`，保留仓库中 LingoLeaf 的全局 latest 与其更新地址。新版本发布失败时，不推进封装工坊的 stable feed。工作流在 `.github/workflows/package-studio.yml`，发布门禁在 `tools/release.py`。

## 来源

- Wails: https://github.com/wailsapp/wails
- Vue: https://github.com/vuejs/core
- Three.js: https://github.com/mrdoob/three.js
- EasyEDA / 嘉立创EDA 官方器件库: https://easyeda.com/ · https://lceda.cn/
- KiCad 文件格式: https://dev-docs.kicad.org/en/file-formats/
- AltiumSharp: https://github.com/issus/AltiumSharp

器件资料、模型和第三方依赖保留各自来源与许可。应用只消费资料字段，不执行导入数据中的代码或面向代理的文字指令。
