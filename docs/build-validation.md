# 0.10.2 诊断与验证记录

日期：2026-11-09。分支：`lingoleaf`。用户现场已确认版本 0.10.1、自动划词 / 兼容取词均开启、翻译剪贴板和 Ctrl+Alt+T 能显示，但自动划词从不弹出。此前两项修复没有解释该现场；本轮随后在原生 Windows 找到并复现了图标层级缺陷，修复已由同一严格用例验证。具体用户机器效果仍待确认。

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
| 正式公开资产 / 旧版升级 | 待发布后验证 |
| 用户机器根因与恢复 | 尚无本机计数报告，未验证解决；不能由云端通过推断 |

## 已确认的验证盲区

此前普通 EDIT 真实鼠标用例绕过 UIA，UIA 选区假对象测试也不代表真实浏览器和 Word。Wine 10 中 ElementFromHandleBuildCache 等真实 API 返回 E_NOTIMPL。新的原生测试独立 STA 窗口泵 / MTA 客户端，读取实际 RichEdit / EDIT 系统控件，不调用 native_edit_selection；真实 TextPattern、选区、几何、空范围和密码属性检查至少有一条实际支持路径必须通过。原生 Windows 不允许跳过；仅检测到 wine_get_version 才明确跳过该项。

该原生测试已在初始 Windows CI 通过；它仍不是对任意 Edge、PDF 或 Word 版本的验证。新工作流另外实际执行 diagnostics_smoke，经真实鼠标验证 hook → controller → native capture → render，不能仅以 cargo test 编译示例记为交互通过。

## 本地诊断范围

仅固定枚举、计数、相对时间、布尔状态与数值系统错误，内存最多 32 条近期决策。回调只做原子增量；没有文本接收 API。显示测试临时使用固定绿叶，不读选区、不发翻译；正常图标设置和缓存随即恢复。报告还记录 DPI、尺寸、显示器数与 DWM 遮蔽状态，不记录指针位置、窗口标题、服务地址、模型或密钥。

只有用户主动复制才写入剪贴板，没有自动文件日志或联网发送。恢复按钮是明确的用户操作：保存自动划词开启、退出本次暂停 / 手动保护、请求重装监听，兼容复制和翻译模式不擅自更改。请求入队与真正安装成功分别记录。

## Wine 界面预检

初始 GNU 预检 SHA `0795adb3efda0009a958b389ac66f027da6e05eaa5138734d8ce7c38d4e53ccb` 的实际诊断 smoke 两次通过。原生 EDIT 的真实拖动观测 HookDown / Up 3、ExternalDown 1、NotifyPosted 2、AppDown / UpAccepted 1、CaptureQueued 1、CaptureSuccess 1，AutoCapture 为 NativeText；RenderSuccess 3，包括两次固定显示和一次真实选区。HookInstallSuccess 3 / Rebind 2，表明显式重装实际完成。零模型 HTTP；报告不包含原文、剪贴板、标题、地址、模型、假密钥和草稿 fixture，恢复后保存的自定义外观不变。

后续候选增加 DWM / DPI 报告与显示自检 60 秒期限，需按最终字节重新确认。这些预检不等于实际用户问题已解决。

## 发布约定

主程序固定 `lingoleaf.exe`；兼容 EXE 字节相同，六字段 schema 1 更新协议不变。正式发行仍需完整 Windows 无过滤测试、真实诊断 smoke、版本与包校验；公开后核对匿名五资产、四 SHA、严格 ZIP 清单、普通 latest 请求和原始旧版升级 / 配置保留。诊断报告不进入发行包。

## 原生交互失败保留

初次启用真实 Windows 诊断 smoke 的 [d238374 分支流水线](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42548479537) 中，库测试 / 严格检查 / 构建通过，但交互步骤失败。这是当前未通过项，不移除该步骤或标记通过。后续增加失败前固定阶段、窗口 / 焦点 / 命中元数据和经隐私检查的报告，并以公开错误注解保留，便于判断是否环境或实现问题。

## 原生失败的具体证据与层级修复

[ba3b993 的原生运行](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42549125119) 记录：固定叶片 UpdateLayeredWindow 成功、IsWindowVisible=1、DWM Cloaked=0、尺寸32×32，但中心点 WindowFromPoint 命中下面的诊断编辑框，测试点击不能关闭叶片。监听已收到按下 / 松开，故不是无输入桌面的假设。失败发生在 DisplayTest.Disabled，尚未进入源控件的真实拖动阶段。

[SetWindowPos 官方规则](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos) 明确：置顶窗口被降为非置顶时，其 owners 与 owned windows 也会变为非置顶。设置页自 0.10.0 降低 result_window 层级，而 result / inline / chip 同属后台 main；图标显示只更新像素，没有像译文窗口一样每次重新确立 HWND_TOPMOST。

补丁在每次图标绘制后 SetWindowPos(HWND_TOPMOST)，保留 NOMOVE / NOSIZE / NOACTIVATE / NOOWNERZORDER，检查实际错误码。原先丢失置顶位时记录 TopmostRestored；诊断页降低阅读窗口后，对仍可见图标也恢复层级但不重新显示隐藏图标。新的严格 native smoke 检查置顶位及不透明中心实际命中，未放宽点击条件。

新 GNU 层级候选为 1217536 字节，SHA `99aa66bc4bf5008684abf7f2c862f956e9013f3ac9ae5417d42d67217821f5bb`。严格 Wine smoke 通过，但 Wine 旧候选本来就保持置顶，不能据此判定原生修复成功；原生结果见下方决定性验证。

## 同一原生用例的修复验证

[13ba3f4 原生 Windows 流水线](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42549965591) 全部通过，包括库测试、严格 Clippy、release 和真实执行的 diagnostics_smoke。两次固定叶片及真正选区图标的 WS_EX_TOPMOST 位、中心 WindowFromPoint 命中均为强断言；没有等待重试图标点击、强制焦点或放宽命中条件。旧 Wine 用例仍通过而原生 Windows 失败的差异现在由明确的 SetWindowPos 层级恢复修复。

这次不是只编译示例：真实鼠标进入 hook → 控制器 → 原生取词 → 图标绘制 / 命中，且自检零 HTTP、报告隐私与原自定义图标恢复均通过。真实系统控件 UIA 测试也通过，仍不能外推所有 Edge / Word / PDF 提供者。最终 tag 和正式公开资产待发布核验。
