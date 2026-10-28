# 安装 Cutcraft CLI

Cutcraft 通过 GitHub Releases 分发可执行文件，不发布到 crates.io。
运行 CLI 不需要 Rust 或 Python。媒体导入、预览和渲染另需 FFmpeg 6.1+ 的 `ffmpeg` 与 `ffprobe`，
且 FFmpeg 构建需要包含 libx264、FFV1 和 drawtext；默认字体取决于本机环境。

安装完成后运行 `cutcraft doctor`，无需创建工程。它检查 FFmpeg / ffprobe 版本和所需组件，并进行小型文字、编码、探测及解码测试；失败仍返回完整 JSON 报告和退出码 1。每项默认超时 10 秒，可用 `--timeout 20` 调整，详见[环境诊断](doctor.md)。

## 支持平台

| 系统 | Release 文件中的 target | 支持范围 |
| --- | --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | glibc 2.39+，在 Ubuntu 24.04 构建和验证；不支持 Alpine/musl |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | glibc 2.39+，交叉编译，QEMU 库测试与 CLI 冒烟验证 |
| Linux ARMv7 | `armv7-unknown-linux-gnueabihf` | glibc 2.39+，32 位 hard-float 用户空间；交叉编译与 QEMU 验证 |
| macOS Apple Silicon | `aarch64-apple-darwin` | macOS 15+ |
| macOS Intel | `x86_64-apple-darwin` | macOS 15+ |
| Windows x64 | `x86_64-pc-windows-msvc` | Windows 10+ / Server 2022+；CI 在 Server 2022 原生测试，ZIP 包 |

v0.1.0 的跨平台重发包含上述六个平台；下载时以实际 Release 附件为准。
Linux ARM 未执行原生硬件媒体回归，不支持 ARMv6、soft-float 或 musl。Windows 暂不分发 ARM64 / x86 包。
macOS 文件暂无 Developer ID 签名和公证；Windows 文件暂无 Authenticode 签名。
通过浏览器下载后可能需要按 macOS 的系统提示允许运行，不要全局关闭系统安全设置。

## 使用安装脚本

安装脚本从 [XGHeaven/cutcraft 的 main 分支](https://github.com/XGHeaven/cutcraft/blob/main/scripts/install.sh) 获取，CLI 二进制与 `SHA256SUMS` 从 GitHub Release 获取：

```sh
curl -fsSL https://raw.githubusercontent.com/XGHeaven/cutcraft/main/scripts/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
cutcraft --version
cutcraft capabilities
```

也可从仓库页面打开 `scripts/install.sh` 并下载 Raw 文件，或在源码仓库中运行 `sh scripts/install.sh`。
脚本修复随 `main` 更新，不要求重新发布 CLI；安装 CLI 仍需要已发布的 Release 和对应平台附件。
默认仓库为 `XGHeaven/cutcraft`；安装其他仓库的构建时可用 `--repo OWNER/REPO` 覆盖。
省略 `--version` 或指定 `--version latest` 时，安装 GitHub 标记的最新正式 Release；
先解析一次具体 `vVERSION` tag，再固定该 tag 下载压缩包和校验和，不按版本号大小自行猜测。
再次运行同一条命令即可更新已有 Cutcraft，目标版本已安装时直接退出，不下载二进制。
默认对齐最新正式版（包括从预发布版切回正式版）；用 `--version` 切换到指定版本，也支持降级。
通过管道传入参数时使用 `sh -s --`，例如指定版本（也支持预发布版本）和目录：

```sh
curl -fsSL https://raw.githubusercontent.com/XGHeaven/cutcraft/main/scripts/install.sh | sh -s -- --version 0.1.0 --bin-dir "$HOME/.local/bin"
```

脚本完整解析后才执行安装，下载中断造成的未闭合脚本不会开始安装。
需要保存脚本或离线安装时，先下载到本地，再用相同参数运行：

```sh
curl -fsSL https://raw.githubusercontent.com/XGHeaven/cutcraft/main/scripts/install.sh -o install.sh
sh install.sh --version 0.1.0
```

脚本识别当前平台，验证同一 Release 的 `SHA256SUMS`，运行版本检查后
安装到 `~/.local/bin`。可用 `--bin-dir /your/bin` 自定义目录；更新自定义目录中的安装时使用相同参数。
`--force` 仅用于相同版本强制重装或替换无法识别的文件、符号链接；目录始终拒绝替换。
识别已有安装时检查目标文件的 `--version` 输出；普通升级不需要 `--force`。
校验和用于发现文件损坏，不等同于独立的数字签名。
在线安装需要 `curl`，解包和校验需要 `tar`、`sha256sum` 或 macOS 自带的 `shasum`；
不需要 Git、jq、Rust 或 Python。当前支持公开 GitHub 仓库；仓库尚无正式 Release、
缺少平台附件、下载失败或校验失败均退出报错，保留已有安装。脚本不自动修改 shell 配置或安装 FFmpeg。

已经下载二进制压缩包及 `SHA256SUMS` 时，可以完全离线安装；离线模式必须指定具体版本：

```sh
sh install.sh --version 0.1.0 \
  --archive cutcraft-v0.1.0-aarch64-apple-darwin.tar.gz \
  --checksums SHA256SUMS --bin-dir "$HOME/.local/bin"
```

也可以手动验证校验和后解压，将包内的 `cutcraft` 放入 PATH。
### Unix 卸载

使用同一安装脚本加 `--uninstall`：

```sh
curl -fsSL https://raw.githubusercontent.com/XGHeaven/cutcraft/main/scripts/install.sh | sh -s -- --uninstall
```

也可离线运行 `sh install.sh --uninstall`。自定义安装目录时附加 `--bin-dir /your/bin`。
卸载只删除该目录中的 `cutcraft`，保留安装目录、其他文件、工程、素材、缓存、FFmpeg 和 Agent 技能；
不修改 shell 配置。未安装时提示后成功退出，不解析 Release 或下载二进制。
无法识别的文件或符号链接需要加 `--force`；删除链接时保留其目标，目录始终拒绝删除。
`--uninstall` 不能与 `--version`、`--repo`、`--archive`、`--checksums` 同用。

### Windows PowerShell

Windows PowerShell 5.1 或 PowerShell 7：

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/XGHeaven/cutcraft/main/scripts/install.ps1 -OutFile install.ps1
.\install.ps1
$env:PATH = "$env:LOCALAPPDATA\Cutcraft\bin;$env:PATH"
cutcraft --version
```

安装入口同样来自 `main`。默认安装最新正式版到 `%LOCALAPPDATA%\Cutcraft\bin`；
用 `-Repo OWNER/REPO`、`-Version VERSION`、`-BinDir DIR`、`-Force` 覆盖设置。
最新版本通过 GitHub API 解析一次，随后 ZIP 与 SHA256SUMS 从同一个 tag 下载。
公开 API 的限流或下载错误会中止安装；可指定版本跳过 API。已有文件默认拒绝覆盖，
验证 SHA-256、解包及运行版本检查完成后才原子替换。不会自动修改 PATH 或执行策略。

离线安装：

```powershell
.\install.ps1 -Version 0.1.0 -Archive .\cutcraft-v0.1.0-x86_64-pc-windows-msvc.zip -Checksums .\SHA256SUMS
```

也可手动核对 SHA-256 后解压
ZIP，将 `cutcraft.exe` 所在目录放入 PATH；卸载删除该文件即可。

## FFmpeg 与字体

macOS（Homebrew）：

```sh
brew install ffmpeg@7
export PATH="$(brew --prefix ffmpeg@7)/bin:$PATH"
```

使用包含 `drawtext` 的构建；部分精简 FFmpeg 包不含文字滤镜。`ffmpeg@7` 是 keg-only，需要将其 bin 目录放到 PATH 前面。
Ubuntu 24.04：`sudo apt-get update && sudo apt-get install ffmpeg fonts-dejavu-core`。
Windows：安装包含上述滤镜/编码器的 FFmpeg Windows 构建，并把 `ffmpeg.exe`、
`ffprobe.exe` 所在目录加入 PATH；CI 使用 Chocolatey 的 `ffmpeg` 包，并优先使用包内真实
可执行文件的目录。部分 Chocolatey .NET 启动器无法处理 Windows 扩展路径，不要让该启动器覆盖真实 FFmpeg 的 PATH 优先级。
用 `ffmpeg -version`、`ffprobe -version` 确认已在 PATH 中。
JSON 工程编辑与校验不要求 FFmpeg。固定字幕字体可通过工程的字体素材引用实现。

缺少程序时，CLI 退出码为 1，stderr JSON 的 `error.code` 为 `MEDIA_TOOL_NOT_FOUND`，
`error.executable` 指出 `ffmpeg` 或 `ffprobe`，`error.hint` 提供当前系统的安装与 PATH 检查说明。
无执行权限使用 `MEDIA_TOOL_PERMISSION_DENIED`；其他进程启动错误使用 `MEDIA_TOOL_START_FAILED`。
程序已启动但媒体处理失败时，保留实际处理错误，不将其误报为未安装。不会自动安装依赖。

## 安装 Agent 技能

推荐使用 [Skills CLI](https://github.com/vercel-labs/skills/blob/main/docs/commands.md)，需要 Node.js / npm：

```sh
npx skills add XGHeaven/cutcraft --skill cutcraft
```

从仓库安装 `skills/cutcraft/` 中的技能及其参考文档，按提示选择 Agent。
默认安装到当前项目；需要跨项目使用时加 `-g`。例如全局安装到 Codex：

```sh
npx skills add XGHeaven/cutcraft --skill cutcraft -g -a codex
```

上述命令也可用于 Windows。技能不包含 Cutcraft CLI 或 FFmpeg，二者仍需单独安装。
仅使用 CLI 二进制时不需要 Node.js / npm。安装后在 Agent 的新会话中使用 `$cutcraft`。

### 离线安装技能

也可下载并校验 Release 中的 `cutcraft-skill-v0.1.0.tar.gz`，解压后进入同名目录：

```sh
sh scripts/install-skill.sh
# 或指定其他兼容 Agent 的技能目录：
sh scripts/install-skill.sh /path/to/agent/skills
```

从源码仓库安装时也可运行同一脚本。此脚本不需要 Node.js / npm，默认复制到
`${CODEX_HOME:-$HOME/.codex}/skills/cutcraft`，不会覆盖已有目录；更新前先备份并移走旧目录。
Windows 离线安装可直接复制解压包内的 `skills/cutcraft` 到 Agent 的技能目录，无需运行 shell 脚本。
