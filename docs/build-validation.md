# 0.7.0 构建验证记录

日期：2026-11-07。开发分支：`lingoleaf`。本记录区分 Linux、Windows 原生 CI、Wine 兼容层与公开更新源；完成发布前会补齐实际结果。0.6.0 历史记录保留在对应 Git 标签。

环境：Debian 13、Rust 1.99.1、Windows GNU 交叉工具链、Wine 10 / Xvfb。目标为 Windows 11 x64，另有 Windows runner 的 MSVC 静态 CRT 构建；Wine 不能代替 Edge 或 Windows 桌面实机验收。

| 检查 | 当前结果 |
|---|---|
| Linux 全 targets 单测 | 57/57 通过 |
| Rust fmt；Linux / Windows GNU 全 targets Clippy `-D warnings` | 通过 |
| Windows GNU release、全 targets 测试编译 / 链接 | 通过 |
| 发行工具、稳定文件名与旧更新协议兼容测试 | Python 14/14 通过 |
| 原生 Windows 全部测试与打包 | 待本次分支 CI 验证 |
| 最终 Wine 设置 / 选区 / 问答 / helper 回归 | 正在验证 |
| 正式发行资产与旧版升级 | 发布后核验；不以标签页面 HTTP 200 代替真实资产 |

## 取消与内存安全

翻译和问答保持各自独立工作线程，WinHTTP 会话改为异步请求。等待时使用条件变量并检查取消和整体截止时间；取消由工作线程关闭自己的请求，不再等待最终关闭通知才处理下一选区。

请求头、请求体和读缓冲区由回调注册 Arc 保留到最终 HANDLE_CLOSING；每次回调也持独立引用，构造失败回收注册引用。每客户端最多 16 个存活请求；取消关闭迟到或失败时不会无界积累缓冲区。测试覆盖无请求发送的关闭、构造失败、上限拒绝及关闭后的槽位回收。

独立网络模块的 Wine 测试已确认：首包阻塞取消约 9.57 ms，同客户端下一请求在旧服务的门闩仍关闭时完成；旧服务结束后活跃槽位回到 0。整体 1 秒截止、普通 JSON、SSE 增量显示、连接复用、问答上下文与安全错误处理通过。

Wine 的 WinHTTP 在“已缓冲 chunk 的末尾，但下一 chunk header 尚未到达”时，异步 ReadData 调用自身仍可能同步阻塞。测试 `winhttp_cancels_a_stalled_sse_body_and_reuses_the_client` 在源码中正常启用，原生 Windows CI 必须实际通过；本地 Wine 回归仅显式过滤这一项，不能将其说成全套通过。没有引入生产环境绕过或为兼容层牺牲增量显示。

## 交互与分发

本轮关注：停止 / 强制重新翻译、停止回答及失败保留草稿、流式选区与上翻位置、保持显示、临时暂停、语言交换、密钥显隐、标题拖动及右键复制。首次 UI 候选已通过扩展设置回归，最终字节仍需下列回归完成后核对。

主文件为 `lingoleaf.exe`、`lingoleaf-windows-x64.zip`；版本化 EXE 是与主 EXE 字节完全相同的旧协议兼容副本。update.json 保持六字段 schema 1 与旧 URL，0.5.0 / 0.6.0 可继续升级；新客户端只额外允许本仓库同标签精确稳定名称。5 项发行资产中 4 个 payload 的哈希列在 SHA256SUMS。

所有网络测试只使用本机公开测试夹具和空 / 假密钥；未调用真实 DeepSeek，不能以模拟结果代表真实模型速度、翻译质量或费用。扫描 PDF / 图片仍不提供 OCR。Windows 的混合 DPI、多屏、实际桌面焦点和 Edge 网页 / PDF 验收仍按验证清单进行。
