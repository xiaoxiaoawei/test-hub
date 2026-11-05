# 版本、发布与更新源

`lingoleaf` 是轻译开发分支。以 `Cargo.toml` 的 `主版本.次版本.修订号` 为准：当前 0.x 阶段新增功能升级次版本，兼容修复升级修订号；每版在 `CHANGELOG.md` 记录用户可见行为与验证状态。

`vX.Y.Z` 标签标记对应源码，必须与 Cargo、锁文件、Windows 资源及更新记录一致。分支、PR、标签和手动 Actions 都执行检查；仅本仓库的版本标签在检查通过后准备 **草稿 Release**。草稿不会进入客户端的已发布更新源，不会自动公开。

## 准备版本

1. 在 `lingoleaf` 完成修改，更新 `Cargo.toml` 并通过 Cargo 同步锁文件中的本项目版本。
2. 同步 `assets/lingoleaf.rc`：数字字段为 `X,Y,Z,0`，`FileVersion` 字符串为 `X.Y.Z.0`，`ProductVersion` 为 `X.Y.Z`。
3. 更新 README、更新记录和 `docs/build-validation.md`，注明构建、Wine 与真实 Windows 的实际结果；未执行项目保持未执行。
4. 使用 Python 3.12 或以上版本完成以下检查；Python 打包工具不需要第三方依赖。

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

Windows CI 使用 MSVC 与 Windows SDK，配置静态 CRT 链接，并核对生成 EXE 的实际 `FileVersion`、`ProductVersion` 与 Cargo 一致。交叉编译可用于构建检查，但不能代替 [Windows 验证清单](windows-validation.md)。CI 的非交互测试也不代表完成手工验收。

打包脚本校验源码版本，拒绝超过 32 MiB、非 PE32+ x64、DLL 或非 GUI EXE 的输入，不执行输入文件。客户端在下载后和替换前另行核对大小、SHA-256、EXE 类型及内嵌版本。文件头或哈希检查不代表代码签名，也不证明实际功能已通过。

## 输出与校验

以 0.5.0 为例，`dist` 包含：

```text
lingoleaf-0.5.0-windows-x64.exe
lingoleaf-0.5.0-windows-x64.zip
update.json
SHA256SUMS
```

ZIP 仅包含程序与明确列出的文档，保留相对链接：

```text
lingoleaf.exe
开始使用.txt
README.md
CHANGELOG.md
docs/releasing.md
docs/build-validation.md
docs/windows-validation.md
docs/images/settings-warm-dark.png  # 文件存在时包含；0.4.0 旧截图
docs/images/appearance.png        # 文件存在时包含；0.4.0 旧截图
docs/images/streaming.png         # 文件存在时包含；0.4.0 旧截图
docs/images/cache.png             # 文件存在时包含；0.4.0 旧截图
docs/images/inline.png            # 文件存在时包含；0.4.0 旧截图
docs/images/selection-icon.png    # 文件存在时包含；0.4.0 旧截图
docs/images/browser-web-0.4.2.png  # 文件存在时包含；0.4.2 测试证据
docs/images/browser-pdf-0.4.2.png  # 文件存在时包含；0.4.2 测试证据
docs/images/settings-dark-0.5.0.png # 文件存在时包含；0.5.0 Wine 模拟源截图
docs/images/settings-day-0.5.0.png  # 文件存在时包含；0.5.0 Wine 模拟源截图
```

不收集用户配置、密钥、缓存、日志、浏览器或测试助手。相同 EXE 与文档在相同 Python/zlib 下生成相同 ZIP：条目顺序、时间戳及权限固定；不同工具链的 EXE 不保证逐字节相同。更新清单是独立发布资产，不放进 ZIP，避免清单与 ZIP 哈希循环依赖。

`SHA256SUMS` 覆盖 EXE、ZIP 和 `update.json`。可核对：

```powershell
Get-FileHash dist/lingoleaf-0.5.0-windows-x64.exe -Algorithm SHA256
Get-FileHash dist/lingoleaf-0.5.0-windows-x64.zip -Algorithm SHA256
Get-FileHash dist/update.json -Algorithm SHA256
```

## 更新清单协议

客户端固定检查：

```text
https://github.com/xiaoxiaoawei/test-hub/releases/latest/download/update.json
```

清单由打包工具生成，字段严格固定，不接受额外下载源：

```json
{
  "schema_version": 1,
  "version": "0.5.0",
  "download_url": "https://github.com/xiaoxiaoawei/test-hub/releases/download/v0.5.0/lingoleaf-0.5.0-windows-x64.exe",
  "sha256": "<由打包脚本生成的64位小写十六进制SHA-256>",
  "size": 0,
  "notes_url": "https://github.com/xiaoxiaoawei/test-hub/releases/tag/v0.5.0"
}
```

示例中的 SHA 与 `size` 是占位说明，不可直接发布；实际清单必须来自同次打包输出。版本仅接受三段稳定版本，下载地址、标签及 EXE 名称必须指向同一版本和仓库；大小限制为 64 字节至 32 MiB。客户端只将更高版本视为更新，不自动降级或选择预发布版本。下载还需符合 HTTPS 与允许的 GitHub 发布资产重定向规则，不允许服务端任意改用其他网站。

更新网络不携带翻译 API Key、配置或所选文字，翻译服务地址不是更新源。首次未发布 Release、缺少清单或网络失败不能被伪报为“已是最新”；具体错误按客户端状态与验证记录核对。

## 标签、CI 产物与草稿

检查和评审通过后，再给目标提交创建版本标签；已公开标签不应移动，修复应递增版本。

```powershell
git switch lingoleaf
git status --short
python tools/release.py --check --tag v0.5.0
git tag -a v0.5.0 -m "LingoLeaf 0.5.0"
git push origin lingoleaf
git push origin v0.5.0
```

Actions 的 Windows 任务生成 `lingoleaf-windows-x64` 产物。普通分支或手动构建不创建 Release；版本标签的草稿发布任务仅在 Windows 与可移植检查都通过后执行，先下载并验证产物校验和，再把 EXE、ZIP、清单和校验和上传到该标签的草稿。

只有该发布任务具有 `contents: write` 权限，其余任务保持只读。重跑可刷新未公开草稿；发现同版本已公开时任务停止，不覆盖现有公开资产，避免同一版本的更新内容漂移。

## 公开后客户端才可发现

发布者需核对草稿的标签、源码提交、EXE 内嵌版本、清单下载地址、大小与 SHA、随包验证记录以及 Windows 验收范围，再将草稿公开。不要把开发分支、Actions 下载链接或草稿链接当作客户端可用更新源。

草稿公开后，应从未登录环境验证固定 `latest/download/update.json` 可访问，并用旧版客户端完成检查、确认下载、校验、确认重启及新版启动。尚未做这些步骤时保持“远端更新源未验收”，不能用本地模拟 HTTP 或 Wine helper 测试代替。若 GitHub 的 latest 指向不是预期稳定版本，应先修正发布设置；不要为补救而放宽客户端地址或版本校验。

当前仍未签名，也没有安装器或后台静默升级服务。应用内更新需要用户确认下载与重启。

## 安装、更新与回退

0.4.2 或更早 EXE 没有本轮更新器，第一次升级至 0.5.0 必须先从系统托盘退出旧版，再手动解压或下载运行本版。关闭设置页只隐藏窗口，不能替代退出程序。

使用 0.5.0 后，启动会后台检查，也可手动检查；发现可用版本先提示下载，校验成功后再确认重启。确认后更新助手等待父进程退出再替换当前 EXE 并启动新版；新版未完成启动确认时尝试回滚旧版。不可写目录不自行请求管理员权限，应手动下载到可写位置。不要在下载或替换期间移动、删除正在使用的程序及更新文件。

配置仍在 `%LOCALAPPDATA%\LingoLeaf`，更新不会要求复制密钥文件。保留服务、模型、DPAPI 密钥、划词模式、兼容取词和图标外观；旧配置缺少 `theme_mode` 时默认为暗夜。兼容取词缺失时保持 `false`，旧总开关关闭状态不变，不隐式启用自动发送或复制。主题按钮只保存主题偏好，API 表单须点保存；保存成功后新设置立即用于后续请求。

发布前对 0.5.0 重点验证：每次启动及重复运行的显示器居中、草稿保留、保存成功/失败提示、无需重启的 API 与模式切换、两种主题即时切换及重启恢复、启动与手动检查、公开源不存在/离线/无更新/有更新、错误下载拒绝、确认重启及失败回滚。继续回归既有划词、剪贴板、图标与混合 DPI，未测项目不标通过，完整步骤见 [Windows 验证清单](windows-validation.md)。

手动回退同样先退出当前版本，再运行保留的旧 EXE。旧版本保存设置可能丢失新字段，先备份当前用户配置；DPAPI 密钥仍与 Windows 用户绑定，备份不代表可以跨机器使用。更新助手保留旧版本用于本次启动失败恢复，不应把所有操作系统错误、断电或用户手动删除都宣传为可以无条件恢复。
