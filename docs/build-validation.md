# 0.9.0 构建验证记录

日期：2026-11-07。分支：`lingoleaf`。本轮为侧栏界面、Markdown 阅读、高清图标、本机朗读和一键检查润色更新。旧版记录保留在对应标签；本记录区分已执行检查、兼容层交互和原生 Windows 实机结果。发行包保留打包时的记录，公开分发与升级结果在发布后补记。

环境：Debian 13、Rust 1.99.1、Windows GNU 交叉工具链、Wine 10 / Xvfb；原生 CI 使用 Windows runner / MSVC 静态 CRT。没有真实翻译密钥，所有模型请求使用本机模拟服务。

| 检查 | 结果 |
|---|---|
| Linux 全 targets 单元测试 | 73/73 通过 |
| Linux 全 targets 严格 Clippy | 通过 |
| Windows GNU release / 全 targets 测试编译 | 构建通过；不当作已运行测试 |
| Windows GNU 全 targets 严格 Clippy | 通过 |
| Markdown 解析与安全转义 | 11 项跨平台测试通过；原生格式待 GUI 检验 |
| RichEdit / 朗读控制 / 高清图标 | 已编译，原生测试与界面正在验证 |
| 三页设置、Markdown / 问答 / 润色交互 | 正在验证 |
| 更新 helper 成功 / 回滚 | 使用 0.9.1 固定夹具，待最终候选回归 |
| 发行工具与版本一致性 | Python 14/14、版本检查通过 |
| 原生 Windows 全测试 / 构建 / 打包 | [分支 CI 通过](https://github.com/xiaoxiaoawei/test-hub/actions/runs/42342686071)；最后换行修复仍待重验 |
| 公开更新源 / 旧版实际升级 | 待发布后验证 |
| Windows 11 / Edge / 混合 DPI / 实际语音发声 | 未执行，不能用 Wine 或无音频测试替代 |

## 实现与测试重点

界面参照资料和布局决定见 [设计说明](design-0.9.0.md)。设置采用侧栏与工作区，正文 16 DIP、辅助文案 13 DIP，工具图标按目标 DPI 像素绘制，标题栏图标与预览在当前窗口 DPI 重建。PerMonitorV2、原生字体及多屏路径分别保留；不以“超清”承诺任意设备上的绝对观感。

译文和问答使用系统 RichEdit50W 与纯 Rust Markdown 解析器，不安装 WebView。验收必须读取原生字符格式，证明标题、粗体、斜体、删除线和等宽代码实际呈现；单纯去除 Markdown 符号不算通过。检查中文 / emoji、列表、引用、表格、未闭合流式片段，保留选区与滚动。HTML、图片和链接不执行、不自动联网；超大或异常复杂内容保留全文并退纯文本。

复制按钮默认纯文本，右键前三项保留选中文字、译文、译文与问答，第四项复制 Markdown 原文；原始译文仍进入模型上下文。润色由用户点击后调用当前模型一次，保留原译文和用户问题草稿；忙时禁用，不自动重复发送。检查失败和停止仍应保留草稿，旧响应不能覆盖新选区。

朗读使用 Windows SAPI 工作线程，不调用翻译 API。开始 / 停止、最新命令覆盖、旧 token 隔离和缺引擎路径需验证；Wine 无本机语音或音频设备时只能验证错误处理，不能冒充已听到声音。Windows 实机需安装对应目标语言的本机语音并独立验收。

## Wine 网络边界

Wine WinHTTP 在已缓冲 chunk 尾部、下一 chunk header 尚未到达时，异步 ReadData 可能同步阻塞。仅允许显式过滤已定位的单项 `translation::winhttp::tests::winhttp_cancels_a_stalled_sse_body_and_reuses_the_client` 并记录数量，不能称作全套通过。原生 Windows CI 必须无过滤执行该用例。

界面测试使用合法 Content-Length SSE，旧响应 body 门闩保持关闭直到后继请求完成，不能靠释放旧响应假装取消成功。原生编辑框菜单、跨进程字符格式探针和真实鼠标输入各自记录；模拟服务不代表真实模型的速度、质量或费用。

## 分发

主文件为 `lingoleaf.exe`，ZIP 为 `lingoleaf-windows-x64.zip`。版本化 EXE 仅作旧更新器兼容副本，需与主 EXE 字节相同；清单保持 schema 1 六字段。发布前检查版本、校验和、ZIP 允许清单和文档链接，发布后实际匿名下载并以旧 0.8.0 验证下载、替换、启动确认及配置保留。标签页面可访问本身不能证明更新可用。

当前最终 GNU 候选为 1148928 字节，SHA-256 `c969f7c5e646d01eb2e21c9521207171a93ea9213f31286b1bd97a8f8a14c998`。该标识仅用于本地交叉构建，不与后续 CI 的 MSVC 字节混用。
