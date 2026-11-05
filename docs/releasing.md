# 版本与打包

`lingoleaf` 是轻译的开发分支。以 `Cargo.toml` 的版本为准，使用 `主版本.次版本.修订号`：当前 0.x 阶段新增功能升级次版本，兼容修复升级修订号。每个版本在 `CHANGELOG.md` 中记录用户能观察到的变化、兼容性和验证状态。

`vX.Y.Z` 标签用于标记对应源码。标签推送、分支推送、PR 或手动运行 Actions 都会执行检查；标签必须与源码版本一致。工作流只生成可下载的构建产物，不创建 GitHub Release，也不自动推送标签。

## 准备一个版本

1. 在 `lingoleaf` 分支完成改动，更新 `Cargo.toml` 的版本，并通过 Cargo 同步 `Cargo.lock` 中的本项目版本。
2. 同步 `assets/lingoleaf.rc` 的四个版本字段：`FILEVERSION`、`PRODUCTVERSION` 为 `X,Y,Z,0`，字符串 `FileVersion` 为 `X.Y.Z.0`，`ProductVersion` 为 `X.Y.Z`。
3. 更新 `CHANGELOG.md`、README 和 `docs/build-validation.md`。验证记录注明平台、工具链和结果；未执行的 Windows 手工项目保持未执行，不能用 Linux 或 Wine 结果代替。
4. 完成以下检查与打包，再提交到开发分支。Python 需要 3.12 或更高版本，无第三方 Python 依赖。

```powershell
python tools/release.py --check
python -m unittest discover -s tools -p test_release.py
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
python tools/release.py --exe target/release/lingoleaf.exe --out-dir dist
```

在 Windows 上使用 MSVC 工具链和 Windows SDK 构建。Linux 交叉编译可用于构建检查，但仍需记录构建来源并完成 [Windows 验证清单](windows-validation.md)。Windows 交互测试需要可交互桌面，Actions 的非交互构建不代表完成手工验收。

脚本会校验 Cargo 清单、锁文件、Windows 资源和更新记录的一致性，拒绝非 x64 PE 输入。它只读取文件头，不运行或验证 EXE 的实际功能，也不证明文件来自某次源码构建。

## 输出与校验

以 0.4.0 为例，`dist` 生成：

```text
lingoleaf-0.4.0-windows-x64.exe
lingoleaf-0.4.0-windows-x64.zip
SHA256SUMS
```

ZIP 内保留以下相对路径，让 README 的文档链接在解压后可用：

```text
lingoleaf.exe
开始使用.txt
README.md
CHANGELOG.md
docs/releasing.md
docs/build-validation.md
docs/windows-validation.md
docs/images/settings-warm-dark.png  # 文件存在时包含
docs/images/appearance.png        # 文件存在时包含
docs/images/streaming.png         # 文件存在时包含
docs/images/cache.png             # 文件存在时包含
docs/images/inline.png            # 文件存在时包含
docs/images/selection-icon.png    # 文件存在时包含
```

打包采用明确文件列表，不收集配置、密钥、缓存、日志或 UI 测试助手。构建前仍应检查源码和文档没有敏感信息。相同 EXE 和文档，在相同 Python/zlib 实现下会得到相同 ZIP：条目顺序、时间戳和权限固定。这不表示不同工具链会生成相同 EXE。

`SHA256SUMS` 包含独立 EXE 和 ZIP 的 SHA-256。可在 PowerShell 中计算并与文件中的对应行比较：

```powershell
Get-FileHash dist/lingoleaf-0.4.0-windows-x64.exe -Algorithm SHA256
Get-FileHash dist/lingoleaf-0.4.0-windows-x64.zip -Algorithm SHA256
```

当前产物未签名，没有安装器、自动更新器或自动升级服务。CI 产物不等于正式发布；公开发布 GitHub Release 应在核对版本、校验和及验证记录后单独执行。

## 标记版本与获取 CI 产物

确认开发分支的检查和评审已通过后，再为目标提交创建标签。以下命令示范 0.4.0；实际操作需与当前版本一致：

```powershell
git switch lingoleaf
git status --short
python tools/release.py --check --tag v0.4.0
git tag -a v0.4.0 -m "LingoLeaf 0.4.0"
git push origin lingoleaf
git push origin v0.4.0
```

发布过的版本标签不应移动；需要修复时递增版本并创建新标签。可在 GitHub Actions 的 **Build, check and package** 运行页面下载 `lingoleaf-windows-x64` 产物。没有标签也可以从该工作流页面手动运行开发分支构建；手动构建不会自动创建标签或 Release。

## 更新与回退

更新时先从系统托盘退出旧版本，确认旧进程已经退出，再将新版解压到新的目录并启动。关闭设置页只会隐藏窗口，不能替代退出程序。

配置保存在当前用户的 `%LOCALAPPDATA%\LingoLeaf`，不随 EXE 目录移动。0.4.0 保留原有服务、模型、DPAPI 密钥和图标样式/颜色，为缺少的字段补充点击翻译模式、32 逻辑像素和圆角形状。原有 `selection_button` 总开关继续有效，关闭状态不会因升级改变；升级不会自动启用直接发送选区的模式。内存翻译缓存不落盘，重启后清空。不要把配置文件或密钥复制到分发包中。

发布前对 0.4.0 额外核对：默认点击模式不在点击前发请求；用户选择直接显示模式后，后续可读取的鼠标选区会自动发送到所配服务；关闭总开关后两种模式均停止。另需验证图标大小、形状、取色取消、无残影及混合 DPI，完整步骤见 [Windows 验证清单](windows-validation.md)。截图若仍为旧版本，应在 README 明确标注，不将旧截图当作新增功能已验收的证据。

回退时同样先退出新版本，再运行保留的旧 EXE。新增的大小、形状和划词模式在旧版中可能不生效；旧版再次保存配置可能丢弃新字段。回退到未验证版本前，先备份当前用户的配置文件；不要将回退视为已验证的迁移流程。DPAPI 密钥与 Windows 用户绑定，备份配置不等于可跨机器使用密钥。
