# 版本、发布与更新源

`lingoleaf` 是轻译开发分支，版本以 `Cargo.toml` 的 `主版本.次版本.修订号` 为准。当前 0.x 阶段新增功能升级次版本，兼容修复升级修订号；每版在 `CHANGELOG.md` 记录行为与验证范围。

`vX.Y.Z` 标签必须与 Cargo、锁文件、Windows 资源和更新记录一致。按当前发行流程，本仓库正常稳定版本标签在 Windows 与可移植检查通过后正式公开并设为 latest。分支、PR 和手动构建只检查并上传产物，不创建发行版。已公开标签和同版本资产不移动、不覆盖，修复应递增版本。

## 准备版本

1. 在 `lingoleaf` 完成修改，同步 `Cargo.toml`、锁文件和 `assets/lingoleaf.rc`。资源数字字段为 `X,Y,Z,0`，`FileVersion` 为 `X.Y.Z.0`，`ProductVersion` 为 `X.Y.Z`。
2. 更新 README、更新记录和 `docs/build-validation.md`，区分逻辑测试、Wine、原生 Windows CI、真实 Windows 桌面与公开更新源。未执行项目保持未执行；设计与界面变化同步 [0.9.0 设计说明](design-0.9.0.md)，新截图注明版本、环境、DPI 和是否使用模拟服务。
3. 使用 Python 3.12 或以上版本与稳定 Rust 完成检查、构建和打包：

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static"
python tools/release.py --check
python -m unittest discover -s tools -p test_release.py
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
python tools/release.py --exe target/release/lingoleaf.exe --out-dir dist
```

Windows CI 使用 MSVC / Windows SDK 与静态 CRT，并从实际 EXE 核对 FileVersion 和 ProductVersion。交叉编译与非交互 CI 不能代替 [Windows 验证清单](windows-validation.md)。

打包工具拒绝超过 32 MiB、非 PE32+ x64、DLL 或非 GUI 的输入，不执行输入文件。客户端在下载后与替换前还会核对精确大小、SHA-256、EXE 类型和嵌入版本。这些检查不代表代码签名或功能验收。

## 文件命名与校验

从 0.7.0 起，给用户的主文件名不带版本号：

```text
lingoleaf.exe
lingoleaf-windows-x64.zip
lingoleaf-0.9.0-windows-x64.exe  # 仅供旧更新器兼容，字节与主 EXE 完全相同
update.json
SHA256SUMS
```

主文件可分别通过发行页，或已公开 latest 下的 `lingoleaf.exe`、`lingoleaf-windows-x64.zip` 下载。版本仍可从程序、Windows 文件属性、Git 标签及更新记录确认。

保留版本化 EXE 是为了兼容 0.5.0 / 0.6.0 客户端严格校验的下载地址，不能只改名删除而使已有更新按钮失效。打包器一次生成两份相同字节，发行任务再次用 `cmp` 确认一致；两份文件均在校验和内。

ZIP 采用固定条目顺序、时间戳和权限，仅包含 `lingoleaf.exe`、开始使用说明、README、CHANGELOG、验证 / 发行文档及允许列入的截图。实际截图名单在 `tools/release.py` 中维护；文档引用新截图时同步加入名单。配置、密钥、缓存、日志、浏览器、测试 DLL 和助手不进入发行包。相同 EXE 和文档在相同 Python / zlib 下产生相同 ZIP，不保证不同工具链 EXE 字节相同。

`update.json` 单独发布，不放进 ZIP。`SHA256SUMS` 覆盖主 EXE、ZIP、版本化兼容 EXE 和更新清单四项：

```powershell
Get-FileHash dist/lingoleaf.exe -Algorithm SHA256
Get-FileHash dist/lingoleaf-windows-x64.zip -Algorithm SHA256
Get-FileHash dist/lingoleaf-0.9.0-windows-x64.exe -Algorithm SHA256
Get-FileHash dist/update.json -Algorithm SHA256
```

## 更新清单协议

已有客户端继续固定检查：

```text
https://github.com/xiaoxiaoawei/test-hub/releases/latest/download/update.json
```

清单仍为 schema 1、六字段，兼容旧客户端；以 0.9.0 为例：

```json
{
  "schema_version": 1,
  "version": "0.9.0",
  "download_url": "https://github.com/xiaoxiaoawei/test-hub/releases/download/v0.9.0/lingoleaf-0.9.0-windows-x64.exe",
  "sha256": "<打包生成的64位小写十六进制SHA-256>",
  "size": 0,
  "notes_url": "https://github.com/xiaoxiaoawei/test-hub/releases/tag/v0.9.0"
}
```

上述 SHA 与大小仅示意，不可用于发布；实际清单必须来自同次打包。`download_url` 继续指向版本化兼容副本，使旧更新器能够验证并升级。

0.7.0 起客户端和打包工具还可严格接受同仓库、同版本标签下的精确 `lingoleaf.exe` 下载地址；本次公开兼容清单仍使用旧名称。两种形式都不允许任意路径、其他仓库、查询参数或 fragment。版本只接受三段稳定数字，大小为 64 字节至 32 MiB；只安装更高版本，不降级，不选择预发行版本。下载继续限制 HTTPS 和 GitHub 发布资产跳转，不因稳定文件名而放宽来源。

更新请求不携带翻译密钥、配置或选区；翻译服务地址不是更新源。不存在公开清单、网络失败和当前版本分别报告，不将失败伪报为已最新。

## 标签与公开发行

检查并核对目标提交后创建标签，例如：

```powershell
git switch lingoleaf
git status --short
python tools/release.py --check --tag v0.9.0
git tag -a v0.9.0 -m "LingoLeaf 0.9.0"
git push --atomic origin lingoleaf v0.9.0
```

发行任务依赖 Windows 与可移植检查，下载同次 Windows 构建的产物、核对 SHA 和两个 EXE 的字节，再公开五项发行资产。仅该任务具有 `contents: write`，其他任务只读。

正常稳定标签没有 Release 时创建正式版本并设为 latest；已有同标签草稿时可更新其资产后公开。发现同版本已公开则停止，不覆盖资产。内部任务 id `draft-release` 保留兼容，实际任务名为 “Publish tagged release”。预发行或非法标签不能绕过版本检查发布。

正式公开后必须独立核验：

1. 未登录访问固定 latest 清单，确认 HTTP 成功且指向目标版本；仅标签页返回 200 不能证明发行资产公开。
2. 实际下载主 EXE、ZIP、兼容 EXE、更新清单和 SHA256SUMS，确认大小 / 哈希、两个 EXE 相同、ZIP 内 EXE 相同且没有测试文件。
3. 使用旧客户端检查、确认下载、校验、确认重启与新版启动，并检查配置保留。区分真实 Windows 联网升级与 Wine 下回放公开原始资产的测试。
4. 将真实结果和限制写入构建记录；本地打包、工作流提交或模拟网络成功不等于公开源已验收。

如果 latest 不是预期稳定版本，应修正发行设置，不能靠放宽客户端地址或版本校验补救。

## 安装、升级与回退

0.4.2 或更早版本没有更新器，首次升级须先从托盘退出旧版，再手动运行新 EXE。0.5.0 及之后可通过既有更新按钮升级；更新助手等待原进程退出后替换当前路径的 EXE，新版未完成启动确认时尝试恢复旧版。应用内更新会保留当前安装路径的文件名；新下载的主 EXE 则统一为 `lingoleaf.exe`。

不可写目录不自动提权，已通过 UAC 提权的进程拒绝自动替换；可退出后手动下载到可写位置。下载或替换中不要移动、删除程序及更新文件。当前未签名，无安装器或后台静默升级服务，下载和重启都由客户端用户确认。

配置仍在 `%LOCALAPPDATA%\LingoLeaf`，升级保留服务、模型、DPAPI 密钥、语言、主题、模式、兼容取词和图标。缺少语言字段时为英语 → 简体中文，兼容取词缺省关闭，明暗缺省暗夜；缺少模板字段时为暖砂金，并保留原有明暗，原关闭总开关保持关闭。暂停划词、保持显示和密钥显隐属于临时交互，不新增持久配置。

0.9.0 发布前重点验证侧栏设置、四套模板的八种明暗组合、100% / 150% / 200% 与混合 DPI 的文字和图标。服务草稿、旧配置迁移、原生输入 / 下拉、键盘焦点、边缘裁切和长时间切换后的 GDI 句柄都应分别检查。

Markdown 检查标题、列表、表格、代码、中英混排、流式未闭合标记、超长内容降级，以及可读纯文本 / Markdown 原文的复制。HTML 不执行、外图不加载、网址不自动打开。朗读应在真实 Windows 有声设备上验证匹配语音、停止与换选区取消；Wine 界面或模拟引擎不代表已听到发声。润色应通过请求记录确认一次显式点击只发一个问答请求，使用当前上下文、保留原译文与输入草稿；真实质量和费用另记。

继续回归既有停止 / 重译、问答、流式选区和滚动位置、保持显示、临时暂停、语言交换、密钥显隐、四项右键复制、剪贴板与更新回滚。不同模板只改变外观，不应使缓存失效、发起翻译或重置自定义图标。Windows 11 系统圆角与 Windows 10 / Wine 降级分别记录，不能由兼容层截图声称实机效果已通过。旧版行为与证据可查相应版本标签，不把历史通过数用于本版。

手动回退也应先退出当前程序，再运行保留的旧 EXE。旧版保存可能丢失新字段；配置备份仍受 Windows 用户范围的 DPAPI 约束。启动失败恢复不代表对断电、用户删除或所有系统错误均可无条件回滚。
