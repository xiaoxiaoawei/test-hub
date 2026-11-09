# 0.10.2 诊断与验证记录

日期：2026-11-09。分支：`lingoleaf`。用户现场已确认版本 0.10.1、自动划词 / 兼容取词均开启、翻译剪贴板和 Ctrl+Alt+T 能显示，但自动划词从不弹出。此前两项修复没有解释该现场；本轮随后在原生 Windows 找到并复现了图标层级缺陷，修复已由同一严格用例验证。用户升级到公开 0.10.2 后反馈“更新后能弹出”，确认该现场恢复。

本版加入本地分段诊断、独立显示测试与显式监听重装，并补真正的 Windows UIA 及监听输入测试。方法见 [划词诊断](selection-diagnostics-0.10.2.md)。旧版本验证保留在相应标签。

| 检查 | 实际结果 |
|---|---|
| 原生 Windows 真实 UIA 集成 | [初始诊断分支 CI](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42547291675) 通过，包含真实系统控件、选区 / 几何 / 密码属性；非浏览器或 Word 实机 |
| Wine 实际 UIA 探针 | EDIT 走原生路径成功；RichEdit 真 UIA E_NOTIMPL。明确不能把 EDIT 成功等同 UIA 成功 |
| Linux / Windows 单元与严格检查 | Linux 87 项与 Python 14 项通过；原生 Windows 全测试、严格 Clippy、构建通过 |
| Wine 真实诊断 UI | 预检通过：真实拖动 → 监听 → 控制器 → 原生取词 → 图标，复制报告无私密 fixture，零 HTTP |
| 显示自检 | 固定 32 DIP 绿叶；点击不发请求；恢复自定义 48 DIP 方形字母图标、颜色和设置 |
| 显式恢复 | 暂停 / 手动模式保持到用户点击恢复；恢复草稿、开启自动划词并请求同线程重装，安装成功计数增加 |
| 更新 helper | 0.10.3 固定夹具替换 / AppReady / 清理及 exit-43 精确回滚成功 |
| 原生 Windows 完整诊断 UI 执行 | 修复前连续失败，层级修复后 [13ba3f4 CI](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42549965591) 严格置顶 / 实际命中 / 真实输入全通过 |
| 最终标签原生 Windows | [v0.10.2 CI](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42550251859) 全绿，完整测试、严格检查、真实诊断 smoke 与发行步骤通过 |
| 正式公开资产 | 匿名下载五资产、四项 SHA、15 项 ZIP 严格清单与普通 latest 请求均通过，详见下方 |
| 0.10.1 → 0.10.2 升级回放 | 原公开旧客户端经 Wine 本地 GET 回放发现 / 下载正式公开新版、校验 / 替换 / 重启成功，468 字节配置逐字节保持，详见下方边界 |
| 用户现场恢复 | 2026-11-09 用户确认升级公开 0.10.2 后“更新后能弹出”；未提供分应用结果或诊断计数，不扩展为所有软件 / 文档类型均通过 |

## 已确认的验证盲区

此前普通 EDIT 真实鼠标用例绕过 UIA，UIA 选区假对象测试也不代表真实浏览器和 Word。Wine 10 中 ElementFromHandleBuildCache 等真实 API 返回 E_NOTIMPL。新的原生测试独立 STA 窗口泵 / MTA 客户端，读取实际 RichEdit / EDIT 系统控件，不调用 native_edit_selection；真实 TextPattern、选区、几何、空范围和密码属性检查至少有一条实际支持路径必须通过。原生 Windows 不允许跳过；仅检测到 wine_get_version 才明确跳过该项。

该原生测试已在初始 Windows CI 通过；它仍不是对任意 Edge、PDF 或 Word 版本的验证。新工作流另外实际执行 diagnostics_smoke，经真实鼠标验证 hook → controller → native capture → render，不能仅以 cargo test 编译示例记为交互通过。

## 本地诊断范围

仅固定枚举、计数、相对时间、布尔状态与数值系统错误，内存最多 32 条近期决策。回调只做原子增量；没有文本接收 API。显示测试临时使用固定绿叶，不读选区、不发翻译；正常图标设置和缓存随即恢复。报告还记录 DPI、尺寸、显示器数与 DWM 遮蔽状态，不记录指针位置、窗口标题、服务地址、模型或密钥。

只有用户主动复制才写入剪贴板，没有自动文件日志或联网发送。恢复按钮是明确的用户操作：保存自动划词开启、退出本次暂停 / 手动保护、请求重装监听，兼容复制和翻译模式不擅自更改。请求入队与真正安装成功分别记录。

## Wine 界面预检

初始 GNU 预检 SHA `0795adb3efda0009a958b389ac66f027da6e05eaa5138734d8ce7c38d4e53ccb` 的实际诊断 smoke 两次通过。原生 EDIT 的真实拖动观测 HookDown / Up 3、ExternalDown 1、NotifyPosted 2、AppDown / UpAccepted 1、CaptureQueued 1、CaptureSuccess 1，AutoCapture 为 NativeText；RenderSuccess 3，包括两次固定显示和一次真实选区。HookInstallSuccess 3 / Rebind 2，表明显式重装实际完成。零模型 HTTP；报告不包含原文、剪贴板、标题、地址、模型、假密钥和草稿 fixture，恢复后保存的自定义外观不变。

后续候选增加 DWM / DPI 报告与显示自检 60 秒期限；下文最终 GNU 候选的严格 Wine smoke 已重新通过。这些预检不等于实际用户问题已解决。

## 发布约定

主程序固定 `lingoleaf.exe`；兼容 EXE 字节相同，六字段 schema 1 更新协议不变。正式发行仍需完整 Windows 无过滤测试、真实诊断 smoke、版本与包校验；公开后核对匿名五资产、四 SHA、严格 ZIP 清单、普通 latest 请求和原始旧版升级 / 配置保留。诊断报告不进入发行包。

## 原生交互失败保留

初次启用真实 Windows 诊断 smoke 的 [d238374 分支流水线](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42548479537) 中，库测试 / 严格检查 / 构建通过，但交互步骤失败。当时保留该失败，不移除该步骤或标记通过。后续增加失败前固定阶段、窗口 / 焦点 / 命中元数据和经隐私检查的报告，并以公开错误注解保留，便于判断是否环境或实现问题。

## 原生失败的具体证据与层级修复

[ba3b993 的原生运行](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42549125119) 记录：固定叶片 UpdateLayeredWindow 成功、IsWindowVisible=1、DWM Cloaked=0、尺寸32×32，但中心点 WindowFromPoint 命中下面的诊断编辑框，测试点击不能关闭叶片。监听已收到按下 / 松开，故不是无输入桌面的假设。失败发生在 DisplayTest.Disabled，尚未进入源控件的真实拖动阶段。

[SetWindowPos 官方规则](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos) 明确：置顶窗口被降为非置顶时，其 owners 与 owned windows 也会变为非置顶。设置页自 0.10.0 降低 result_window 层级，而 result / inline / chip 同属后台 main；图标显示只更新像素，没有像译文窗口一样每次重新确立 HWND_TOPMOST。

补丁在每次图标绘制后 SetWindowPos(HWND_TOPMOST)，保留 NOMOVE / NOSIZE / NOACTIVATE / NOOWNERZORDER，检查实际错误码。原先丢失置顶位时记录 TopmostRestored；诊断页降低阅读窗口后，对仍可见图标也恢复层级但不重新显示隐藏图标。新的严格 native smoke 检查置顶位及不透明中心实际命中，未放宽点击条件。

新 GNU 层级候选为 1217536 字节，SHA `99aa66bc4bf5008684abf7f2c862f956e9013f3ac9ae5417d42d67217821f5bb`。严格 Wine smoke 通过，但 Wine 旧候选本来就保持置顶，不能据此判定原生修复成功；原生结果见下方决定性验证。

## 同一原生用例的修复验证

[13ba3f4 原生 Windows 流水线](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42549965591) 全部通过，包括库测试、严格 Clippy、release 和真实执行的 diagnostics_smoke。两次固定叶片及真正选区图标的 WS_EX_TOPMOST 位、中心 WindowFromPoint 命中均为强断言；没有等待重试图标点击、强制焦点或放宽命中条件。旧 Wine 用例仍通过而原生 Windows 失败的差异现在由明确的 SetWindowPos 层级恢复修复。

这次不是只编译示例：真实鼠标进入 hook → 控制器 → 原生取词 → 图标绘制 / 命中，且自检零 HTTP、报告隐私与原自定义图标恢复均通过。真实系统控件 UIA 测试也通过，仍不能外推所有 Edge / Word / PDF 提供者。

最终 [v0.10.2 标签流水线](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42550251859) 重新完整通过。公开的成功注解保留两次显示测试与真实选区图标在诊断页前后的 `topmost=true`、中心实际命中一致；`CaptureSuccess: 1`、`RenderSuccess: 3`，以及三次 `Render: TopmostRestored`。标签与公开资产冻结，不随后续验证文档更新改写。

## 正式公开资产核验

已匿名下载 v0.10.2 的五个发行资产。正式 MSVC 主程序为 **1188864 字节**，SHA-256 **`559d9151be974bb49f979a866bc843fc59b7cc6847ea73fb60e9360bd51ab83c`**，是 Windows x64 GUI PE，只依赖 Windows 系统 DLL。它与兼容版本化 EXE、ZIP 内程序逐字节相同，与上文 GNU 本地候选分别记录。

`update.json` 六字段严格校验通过，版本、大小与 SHA 对应正式 EXE；四项 SHA256SUMS 全部相符。ZIP 为严格允许的 15 项，无重复或额外文件、CRC 通过，文档与冻结标签源码一致（仅容许 Markdown checkout 换行差异）。不带特殊请求头的普通 latest 清单和 EXE 均返回 HTTP 200，与标签资产逐字节相同。

## 原公开 0.10.1 客户端升级回放

旧程序取自原公开发行，1174016 字节、SHA-256 `1a97524aedc92310864d0e5cc5ddeaab645496872f3a95121cbedd9722574feb`。在隔离配置与目录中运行，旧 GUI 确认发现 0.10.2、下载完成后再次核验大小与 SHA，点击重启更新。新进程实际启动为 0.10.2，GUI 可用且前台，更新状态显示已是最新，最终程序与上文真实公开 MSVC 字节相同。

原有 468 字节配置完全相同（前后 SHA-256 `26c41851a02f6c72d6e080883d7bdad699b39d5999a75de7fb210743d3552cc0`）；另在新 GUI 确认自动检测 → 日语、直接翻译、兼容取词、37 秒超时、星云紫白天模式及自定义圆形字母 48 DIP / `#D6A575` 均保留。AppReady 成功路径完成，backup / stage / ticket 残留均为 0，无回滚，测试进程正常退出。

此回放在 Wine 使用仅 GET 的本地传输夹具，送入未改动的真实公开清单与 MSVC EXE，没有更改更新校验、令牌或替换保护；它验证旧客户端 GUI 及替换重启流程，不是原生 Windows 直接访问 GitHub 的 WinHTTP HTTPS 测试。真实匿名 GitHub HTTPS 五资产与普通 latest 请求由上一节独立验证。测试使用空密钥，没有调用翻译或朗读。
