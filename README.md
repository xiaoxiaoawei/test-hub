# 轻译 · LingoLeaf

面向 Windows 的轻量英文划词翻译工具。使用 Rust 和原生 Win32 界面，托盘常驻，选中英文后点击划词按钮或使用全局快捷键，调用用户配置的翻译 API 显示中文译文。界面采用简洁的暖色暗色主题，不依赖浏览器或 WebView。

当前版本 **0.3.0**，代码在 `lingoleaf` 开发分支维护。新增可自定义的图标样式和颜色、流式译文、内存缓存及连接复用。更新时先从托盘退出旧版本，再解压运行新版；已保存的 API 配置继续使用。版本变化见 [更新记录](CHANGELOG.md)，打包与迭代流程见 [版本管理说明](docs/releasing.md)。

![暖色暗色设置页，Wine 兼容层截图](docs/images/settings-warm-dark.png)

截图来自兼容层；Windows 11 标题栏通过 DWM 应用暗色，系统外观可能不同。

当前是开发测试版。目标平台是 Windows 11 x64；Windows 10 兼容性需要单独验证。跨软件取词、焦点、剪贴板、DPI 和资源占用须在 Windows 实机或虚拟机上验收，构建通过不能代替这些检查。验收记录见 [Windows 验证清单](docs/windows-validation.md)。

## 使用

1. 启动 `lingoleaf.exe`，在系统托盘中找到“轻译”。
2. 首次运行会打开设置页。填写服务地址、模型、API Key 和超时时间，点击“测试连接”验证后保存；之后也可从托盘菜单打开设置。
3. 在其他程序中选中英文，点击出现的划词按钮（默认“译”）；也可以按 **Ctrl + Alt + T** 获取选区并翻译。
4. 如果目标软件无法读取选区，可以自行复制文本，再使用托盘菜单中的“翻译剪贴板”。该菜单只读取剪贴板，不发送复制按键。
5. 从结果浮窗复制译文；浮窗获得焦点后可按 Esc 关闭。关闭设置或浮窗会保留托盘进程，退出程序请使用托盘菜单。

取词优先使用 Windows UI Automation。是否可读取选区取决于目标软件的辅助功能支持；快捷键取词可尝试受限的复制回退，但不能保证所有控件都适用。管理员窗口、扫描 PDF、图片、自绘画布、游戏及远程桌面内部的文字不在首版保证范围内。程序不应为获取文字默认要求管理员权限。

划词按钮默认开启，可以在设置中关闭。鼠标松开后，程序仅尝试通过 UI Automation 读取选区；这一过程不会模拟复制或发送翻译请求，点击划词按钮后才翻译。无法读取选区的软件可以使用快捷键或“翻译剪贴板”菜单。

快捷键的模拟复制回退仅在原剪贴板为空或能安全备份的单一 Unicode 文本格式时尝试；图片、富文本和多格式内容会保留，并提示手动复制。不同软件可能额外提供文本格式，因此“翻译剪贴板”是更广泛可用的主动回退。

### 图标外观

点击设置页底部的“图标”，或托盘菜单的“图标外观”，选择 **叶片 / 译字 / 字母 A** 样式和颜色。点击“应用”后更新托盘、程序窗口图标和划词按钮，重启后保留；旧配置会使用默认的暖色“译”图标。

![图标样式和颜色预览，Wine 兼容层截图](docs/images/appearance.png)

这一选项修改运行中的图标。Windows 资源管理器展示的 EXE 文件图标来自构建时嵌入的固定资源，不会随设置改写。

### 翻译响应与缓存

- **增量显示**：服务返回兼容 SSE 流式响应时，译文逐段显示，首段可以早于整段翻译完成；返回标准 JSON 的服务会在完整响应后显示。
- **重复翻译**：成功结果只保存在内存中，最多 64 项，原文与译文合计最多 512 KiB，每项 10 分钟过期。相同服务、模型和密钥下再次翻译相同文本，缓存命中后直接显示结果，不再次调用 API；重启会清空缓存。
- **连续操作**：合并重复请求，只保留最新待处理翻译；过期结果不会覆盖当前译文。在网络操作之间检查取消状态，停止处理已过期的流式内容。
- **连接复用**：同一服务复用 HTTP 会话和连接，减少重复建立连接的开销；网络和 UI 分别在工作线程与界面线程处理。

首次翻译的时间仍取决于网络、服务排队、模型和文本长度。流式显示改善首段等待感受，不保证缩短服务生成完整译文的时间；同步网络阶段需要返回或超时后才能响应取消，超时按网络阶段生效，不是严格的整体截止时间。当前验证结果见 [构建验证记录](docs/build-validation.md)，真实 Windows 和实际服务速度仍需按 [验证清单](docs/windows-validation.md) 测量。

自动将选中文字上传并翻译、OCR 框选翻译、离线模型、自动更新和云同步尚未提供。

## 配置翻译 API

当前支持 **OpenAI Chat Completions 兼容协议**，翻译请求使用 `stream: true`，可处理兼容 SSE 流及普通 JSON 响应。服务需接受该请求参数；专用翻译服务或其他协议变体需要各自适配。

| 设置 | 说明 |
|---|---|
| 服务 | 选择 DeepSeek、OpenAI 或自定义兼容服务；切换预设会清空密钥，请填写对应服务的密钥 |
| 服务地址 | API 基础地址，通常包含 `/v1`，例如 `https://api.openai.com/v1`；程序在其后使用 `/chat/completions` |
| 模型 | 服务实际提供的模型名称，例如 `gpt-4o-mini`；模型是否可用由服务和账户决定 |
| API Key | 对应服务的密钥；不要求认证的本地服务可以留空 |
| 超时 | 请求的超时秒数，用于限制网络等待 |
| 划词按钮 | 开启或关闭鼠标选中文字后的翻译按钮 |
| 图标外观 | 选择叶子、译、A 样式及图标颜色 |

远程服务必须使用 HTTPS；本机 loopback 地址可使用 HTTP，以便连接本地兼容服务。URL 不应包含用户名、密码、查询参数或 fragment。改变服务地址时，应确认密钥属于该服务。

### 使用 DeepSeek

如果密钥是在 [DeepSeek 开放平台](https://platform.deepseek.com/) 创建的，选择 **DeepSeek** 预设，然后填写：

| 设置 | 内容 |
|---|---|
| 服务地址 | `https://api.deepseek.com/v1` |
| 模型 | `deepseek-chat` |
| API Key | 自己在 DeepSeek 开放平台生成的有效密钥 |
| 超时 | 可先使用预设的 30 秒 |

点击“测试连接”，成功后保存。`/v1` 是兼容 API 路径；应用会组成 `https://api.deepseek.com/v1/chat/completions`。官方模型仓库中的 [OpenAI 兼容调用示例](https://github.com/deepseek-ai/DeepSeek-V2#7-api-platform) 使用这一服务地址与 `deepseek-chat`。账号实际可用模型及额度仍以开放平台为准。

“订阅”可能指网页聊天服务，也可能指第三方提供的 API 套餐。仅凭密钥前缀不能判断服务来源或额度；聊天订阅不应直接视为已包含官方 API 额度。如果购买的是第三方订阅或中转服务，请选择自定义服务，并使用它提供的服务地址、模型名和密钥，不能直接套用官方地址。

切换预设只填入地址、模型等非敏感配置，不会内置共享密钥，也不会覆盖已保存的设置，直到点击保存。已公开发送过的密钥应在对应服务后台撤销，并重新生成后填写；不要将密钥发到聊天、issue 或代码仓库。

翻译方向固定为英文到简体中文，输出按纯文本显示。请求会把所选文字发送到配置的服务，费用、数据处理方式及模型可用性由该服务决定；应用不提供共享密钥，也不需要自建中转服务。

设置保存在当前用户的 `%LOCALAPPDATA%\LingoLeaf` 下。API Key 使用当前用户范围的 Windows DPAPI 保护，不应以明文写入普通配置、日志或仓库。DPAPI 用于保护静态存储，不能抵抗已取得同一用户权限的恶意软件；也不要假定配置可以连同密钥直接迁移到另一台机器。

## 构建

Windows 上安装稳定版 Rust（MSVC 工具链）及 Visual Studio Build Tools 的“使用 C++ 的桌面开发”组件和 Windows SDK，然后在仓库根目录运行：

```powershell
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
.\target\release\lingoleaf.exe
```

可执行文件位于 `target\release\lingoleaf.exe`。使用 Python 3.12 或以上版本生成带文档的 ZIP、独立 EXE 和校验和：

```powershell
python tools/release.py --check
python -m unittest discover -s tools -p test_release.py
python tools/release.py --exe target/release/lingoleaf.exe --out-dir dist
```

输出文件包含版本号，例如 `dist\lingoleaf-0.3.0-windows-x64.zip`，校验和保存在 `dist\SHA256SUMS`。包内不包含用户配置、密钥、缓存或测试助手。当前没有签名安装器或自动更新器，公开分发前仍需完成 Windows 验收；操作步骤见 [版本管理说明](docs/releasing.md)。

在可交互的 Windows 桌面上，可先退出正在使用的轻译，再运行原生 UI 回归检查：

```powershell
cargo run --locked --example ui_smoke -- target\release\lingoleaf.exe
```

检查会使用独立临时配置启动测试实例，将焦点保持在设置顶层窗口，并验证空闲 CPU、服务预设、切换密钥清空及正常退出，不会调用翻译 API，也不会操作已经运行的用户实例。Wine 可以用于兼容层回归，结果需与 Windows 实机验收区分。

Linux 可以运行可移植配置、请求和响应处理的测试：

```sh
cargo test --locked --all-targets
```

Windows 原生交互不能在 Linux 测试中得到验证。分支推送、PR、`vX.Y.Z` 标签及手动运行都可触发 CI；Windows 任务执行格式检查、测试、Clippy、版本核对和 release 打包，上传 `lingoleaf-windows-x64` 产物。Linux 任务运行可移植测试和打包脚本测试。CI 不使用真实 API Key，也不自动创建 GitHub Release；构建成功不代表已经完成交互验收。

当前云端构建与测试记录见 [构建验证记录](docs/build-validation.md)。

## 开发边界

- UI 线程只处理窗口交互；UI Automation 和网络请求放在工作线程。
- 优先读取真实选区，不将整个控件内容冒充选中文字。
- 剪贴板回退必须保留用户数据；无法安全恢复时，应提示用户自行复制。
- 不持久化翻译历史；缓存仅在内存中保留有限时间，日志不得包含 API Key 或所选原文。
- API 错误、超时、无选区及不支持的目标软件需要明确反馈。
- 体积、内存、空闲 CPU 和 Windows 兼容性均以实测为准，不把框架选型视作性能结果。

本项目尚未声明开源许可证。
