# 安装 Cutcraft CLI

Cutcraft 通过 GitHub Releases 分发可执行文件，不发布到 crates.io。
运行 CLI 不需要 Rust 或 Python。媒体导入、预览和渲染另需 FFmpeg 6.1+ 的 `ffmpeg` 与 `ffprobe`，
且 FFmpeg 构建需要包含 libx264、FFV1 和 drawtext；默认字体取决于本机环境。

## 支持平台

| 系统 | Release 文件中的 target | 支持范围 |
| --- | --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | glibc 2.39+，在 Ubuntu 24.04 构建和验证；不支持 Alpine/musl |
| macOS Apple Silicon | `aarch64-apple-darwin` | macOS 15+ |
| macOS Intel | `x86_64-apple-darwin` | macOS 15+ |

当前不分发 Windows 或 Linux ARM 二进制。macOS 文件暂无 Developer ID 签名和公证；
通过浏览器下载后可能需要按 macOS 的系统提示允许运行，不要全局关闭系统安全设置。

## 使用安装脚本

安装脚本从 [XGHeaven/cutcraft 的 main 分支](https://github.com/XGHeaven/cutcraft/blob/main/scripts/install.sh) 获取，CLI 二进制与 `SHA256SUMS` 从 GitHub Release 获取：

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://raw.githubusercontent.com/XGHeaven/cutcraft/main/scripts/install.sh -o install.sh && \
  sh install.sh
export PATH="$HOME/.local/bin:$PATH"
cutcraft --version
cutcraft capabilities
```

也可从仓库页面打开 `scripts/install.sh` 并下载 Raw 文件，或在源码仓库中运行 `sh scripts/install.sh`。
脚本修复随 `main` 更新，不要求重新发布 CLI；安装 CLI 仍需要已发布的 Release 和对应平台附件。
默认仓库为 `XGHeaven/cutcraft`；安装其他仓库的构建时可用 `--repo OWNER/REPO` 覆盖。
省略 `--version` 或指定 `--version latest` 时，安装 GitHub 标记的最新正式 Release；
先解析一次具体 `vVERSION` tag，再固定该 tag 下载压缩包和校验和，不按版本号大小自行猜测。
指定版本（包括预发布版本）可用 `sh install.sh --version 0.1.0`。

脚本识别当前平台，验证同一 Release 的 `SHA256SUMS`，运行版本检查后
安装到 `~/.local/bin`。可用 `--bin-dir /your/bin` 自定义目录；默认拒绝覆盖，升级时加
`--force`。校验和用于发现文件损坏，不等同于独立的数字签名。
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
卸载只需删除安装目录中的 `cutcraft`；工程和素材保留在原位置。

## FFmpeg 与字体

macOS（Homebrew）：

```sh
brew install ffmpeg@7
export PATH="$(brew --prefix ffmpeg@7)/bin:$PATH"
```

使用包含 `drawtext` 的构建；部分精简 FFmpeg 包不含文字滤镜。`ffmpeg@7` 是 keg-only，需要将其 bin 目录放到 PATH 前面。
Ubuntu 24.04：`sudo apt-get update && sudo apt-get install ffmpeg fonts-dejavu-core`。
用 `ffmpeg -version`、`ffprobe -version` 确认已在 PATH 中。
JSON 工程编辑与校验不要求 FFmpeg。固定字幕字体可通过工程的字体素材引用实现。

缺少程序时，CLI 退出码为 1，stderr JSON 的 `error.code` 为 `MEDIA_TOOL_NOT_FOUND`，
`error.executable` 指出 `ffmpeg` 或 `ffprobe`，`error.hint` 提供当前系统的安装与 PATH 检查说明。
无执行权限使用 `MEDIA_TOOL_PERMISSION_DENIED`；其他进程启动错误使用 `MEDIA_TOOL_START_FAILED`。
程序已启动但媒体处理失败时，保留实际处理错误，不将其误报为未安装。不会自动安装依赖。

## 安装 Agent 技能

另外下载并校验 `cutcraft-skill-v0.1.0.tar.gz`，解压后进入同名目录：

```sh
sh scripts/install-skill.sh
# 或指定其他 Agent 的技能目录：
sh scripts/install-skill.sh /path/to/agent/skills
```

技能默认安装到 `${CODEX_HOME:-$HOME/.codex}/skills/cutcraft`，不会覆盖已有安装。
技能包不包含 CLI 或 FFmpeg；更新技能前先备份并移走已有目录。
