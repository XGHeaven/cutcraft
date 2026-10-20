# Cutcraft

面向 AI Agent 的可编程非线性剪辑工具。Rust CLI + 可复用 Rust 核心，使用 JSON 工程、精确有理秒、原子领域命令和可审计历史。

采用 **Cutcraft Spec 0.1.0**，支持动画、转场、精确时间映射及原生图片序列；内部 0.3/0.4/0.5 项目保留兼容路径，不隐式升级。使用方式见 [动画与变速](docs/animation-and-retime.md)、[图片序列](docs/image-sequences.md)，协议与 Schema 见 [`spec/`](spec/)。**领域模型与基础媒体后端已经可运行；这还不是完整 NLE 渲染器。** 支持范围和明确拒绝的编辑／渲染情况见 [实现状态](docs/implementation.md)。

## 安装与运行

CLI 通过 GitHub Releases 分发，不发布到 crates.io。构建流程覆盖 Linux x86_64 / ARM64 / ARMv7、
macOS Apple Silicon / Intel 和 Windows x64。具体下载文件以 Release 附件为准。
从仓库 `main` 分支下载安装脚本，再由脚本安装 [最新正式版 CLI](https://github.com/XGHeaven/cutcraft/releases)：

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://raw.githubusercontent.com/XGHeaven/cutcraft/main/scripts/install.sh -o install.sh && \
  sh install.sh
```

默认安装到 `~/.local/bin`。可用 `--version 0.1.0` 固定版本、`--force` 更新已有安装；
也支持本地压缩包离线安装，详见 [安装说明](docs/install.md)。
二进制安装不需要 Rust 或 Python；媒体操作需要外部 FFmpeg / ffprobe。
Windows 使用 `scripts/install.ps1`，平台要求和 PowerShell 安装方式见 [安装说明](docs/install.md#windows-powershell)。

发布流程和本地演练见 [发布指南](docs/releasing.md)，版本变化见 [Changelog](CHANGELOG.md)。

## 从源码构建

需要 Rust 1.86+。素材探测、导入、导出和视频预览另需 `ffmpeg` / `ffprobe`；纯 JSON 编辑和校验不依赖 FFmpeg、Python、Node 或网络。

```sh
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo install --locked --path .
```

## 安装 Agent 技能

仓库提供可独立安装的 [`skills/cutcraft/`](skills/cutcraft/SKILL.md)，包含按任务分类的功能索引与操作指导：

- [基础编辑与项目管理](skills/cutcraft/references/editing-guide.md)：素材导入、多序列／多轨道、裁剪／切分／波纹编辑、A/V 联动、音频混合、字幕与标题、图片与背景、嵌套序列、调整层、批量事务、标记、历史撤销、校验、预览和导出。
- [动画与效果](skills/cutcraft/references/motion-and-effects.md)：属性动画、变速与时间映射、转场、调色、RGB 曲线和 LUT。

技能目录可以随仓库分发，也可以单独复制；技能不是 CLI 二进制，媒体操作仍需安装 Cutcraft 和 `ffmpeg` / `ffprobe`。文档区分专用 CLI 命令、JSON 领域命令和当前未实现的能力。

从仓库根目录安装到 Codex：

```sh
cargo install --locked --path .
./scripts/install-skill.sh
```

默认安装到 `${CODEX_HOME:-$HOME/.codex}/skills/cutcraft`。如需其他兼容 Agent 的技能目录，可显式指定其父目录：

```sh
./scripts/install-skill.sh /path/to/agent/skills
```

安装脚本只复制技能，不安装依赖，也不覆盖已有技能。已有安装会报错并保留原文件；更新时请先备份并移走旧的 `cutcraft` 技能目录，再安装新版。安装后在新会话中使用，例如：

> 使用 $cutcraft，把指定文件夹中的视频剪成 30 秒短片，项目和导出都放在我指定的目录中。

技能内容会进入 Git；`local-examples/` 下的演示项目和媒体仍保持忽略。

## 创建剪辑项目

```sh
cutcraft init my-film.cut --name 'My film'
cutcraft -p my-film.cut import /absolute/path/video.mp4 --managed --append --color-space rec709
cutcraft -p my-film.cut inspect
cutcraft -p my-film.cut validate --level media
cutcraft -p my-film.cut preview
cutcraft -p my-film.cut render --output output/final.mp4
```

`--managed` 复制原素材到工程的 `assets/`，否则保存可移植的相对文件 URI。`--append` 为每个可播放 stream 新建轨道与片段，并将视音频组成 LinkGroup。导入和追加分别产生一个 Operation；追加失败时已导入的素材仍保留。图片默认追加 5 秒；视频文件内嵌的封面图会识别为图片 stream，但不会被 `--append` 当成独立片段追加。

不需要准备素材的演示：

```sh
cargo build --locked
python3 examples/demo.py  # 创建 local-examples/demo.cut，并输出预览 MP4 路径
```

生成的演示项目统一放在 `local-examples/`，整个目录由 Git 忽略；`examples/` 只保留可复现的示例代码。

项目布局：

```text
my-film.cut/
  cutcraft.json       # canonical 工程快照
  history.jsonl      # applied / rejected 事件与 inverseCommands
  assets/            # 受管原素材
  cache/             # 可重建预览缓存，可删除，使用时自动重建
  output/            # 导出
  .cutcraft/         # 进程锁与崩溃恢复日志，不是可删除缓存
```

新项目不再创建 `working/`。旧项目中的 `working/` 保留原样；其中可能含有被引用的素材或辅助文件，不自动迁移或删除。`cache/` 仅用于可重建缓存，不存放原素材或编辑输入。

## Agent 接口

成功结果为 stdout JSON，失败为 stderr JSON 和退出码 1；命令行语法错误为退出码 2。`--help` 保持人类可读。`--project` / `-p` 指向工程目录，可放在子命令前后。生成的 ID、操作和 revision 均在 JSON 结果中，无需解析自然语言。

```sh
cutcraft schema project
cutcraft schema operation
cutcraft schema generator  # 当前内建生成器的参数约束
cutcraft id item
cutcraft capabilities
cutcraft -p my-film.cut history
```

`command` 接受单个领域命令或 batch，并生成 Operation envelope。`apply` 接受完整 Operation（包含 `operationId`、`baseRevision` 和 actor），适合并发 Agent。文件名 `-` 表示 stdin。

```sh
printf '%s\n' '{"type":"project.update","changes":{"name":"Launch film"}}' \
  | cutcraft -p my-film.cut command - --base-revision 2
```

```sh
cutcraft -p my-film.cut apply operation.json --dry-run
cutcraft -p my-film.cut apply operation.json
cutcraft -p my-film.cut undo
```

`--dry-run` 对编辑命令进行完整验证但不写快照或历史。不适用于导入／渲染／初始化。成功 operation 的 revision 加一，batch 也只加一；失败 operation 不改变快照。有效 envelope 的语义拒绝会记录 rejected event。格式错误的 envelope 无法生成符合 Event Schema 的事件，因此只返回错误。重复 `operationId` 被拒绝，重试前可查询 history；更新后的请求需要新 ID。

锁定轨道的内容、排序和关联修改被拒绝；只有显式 `--override-locks` 可以覆盖。不要把该权限写进 JSON 工程。

## 时间与编辑

所有保存的编辑时间均为 `{ "num": 1001, "den": 30000 }` 有理秒。内部使用任意精度整数计算；序列化范围为有符号 64 位整数，超出时报错。写入时自动约分，零写为 `0/1`。CLI 时间支持整数秒、有限小数秒、`12.4s`、`1001/30000`，暂不接受帧号和 SMPTE timecode。

```sh
cutcraft -p my-film.cut track add --kind video --name V2
cutcraft -p my-film.cut item move ITEM_ID --start 3/2 --ripple insert
cutcraft -p my-film.cut item trim ITEM_ID --start 0 --duration 3 \
  --source-start 1 --source-duration 3
cutcraft -p my-film.cut item split ITEM_ID --at 1001/30000
cutcraft -p my-film.cut marker --name 'Review here' --at 2.5s
cutcraft -p my-film.cut sample --at 1001/30000
```

编辑命令默认操作 root sequence；`--sequence SEQUENCE_ID` 指定其他序列。`sample` 输出当前启用的层、精确源时间和嵌套层结构，不解码视频。反向播放用 `exclusive-upper` 表示源区间右侧的单侧极限，避免浮点 epsilon。

素材和片段查询默认覆盖所有序列，返回实际 ID 和定位信息，无需用 jq 遍历项目：

```sh
cutcraft -p my-film.cut asset search '喜字'
cutcraft -p my-film.cut asset search --name 'IMG_' --path '云南'
cutcraft -p my-film.cut asset info ASSET_ID
cutcraft -p my-film.cut asset usages ASSET_ID
cutcraft -p my-film.cut item list --asset ASSET_ID
cutcraft -p my-film.cut item search '片头'
cutcraft -p my-film.cut item list --sequence SEQUENCE_ID --at 3/2
cutcraft -p my-film.cut item info ITEM_ID
cutcraft -p my-film.cut sequence list
cutcraft -p my-film.cut track list --sequence SEQUENCE_ID
cutcraft -p my-film.cut sequence usages CHILD_SEQUENCE_ID
```

支持中文/带空格路径、素材类型/标签/未使用筛选、媒体/LUT/字体引用追踪。查询只读，时间始终属于返回的序列；完整匹配规则和嵌套定位说明见 [搜索与定位](docs/search-and-locate.md)。

所有规范中的 19 种非 batch 命令均可通过 `command` / `apply` 使用：

- `project.update`
- `asset.put/remove`、`sequence.put/remove`
- `track.put/remove/reorder`
- `item.put/remove/move/trim/split`
- `transition.put/remove`、`link.put/remove`、`marker.put/remove`
- `batch` 将这些命令原子提交

Effects 是 Item 的有序组成部分，通过 `item.put` 修改。`*.put` 完整替换实体，并保留提交对象中的未知扩展；只有 `extensions` / `metadata` / 插件参数可以含扩展 JSON。未知核心字段会被拒绝。

## 校验、预览与导出

原生可编辑标题和字幕可直接通过 CLI 创建，支持字体素材、字号、RGBA 颜色和基本对齐：

```sh
cutcraft -p my-film.cut item text --track VIDEO_TRACK_ID --duration 3 --text '你好，Cutcraft' --font-size 64 --position center
cutcraft -p my-film.cut item caption --track SUBTITLE_TRACK_ID --duration 3 --text-file captions.txt --font-size 32 --color '#ffffffcc'
cutcraft -p my-film.cut preview --progress human
cutcraft -p my-film.cut render --progress json --output output/final.mp4
```

`--text` 与 `--text-file` 二选一；UTF-8 文件里的换行原样存入工程。`--font-asset` 引用已有字体素材，
`--align left|center|right`、`--position top|center|bottom` 控制布局；自动换行、富文本和文本框约束尚未实现。
进度默认 `auto`：stderr 为终端时显示，否则保持安静；`human`、`json`、`none` 可显式选择。
JSON 进度逐行写入 stderr，stdout 始终只返回最终结果。详细阶段、ETA 与文字能力见 [文字与渲染进度](docs/text-and-progress.md)。

图片序列可以直接导入为一条视频素材；新建的 0.1.0 项目支持以下命令：

```sh
cutcraft -p my-film.cut import-sequence /path/to/frames --pattern 'frame_%04d.png' --fps 24 --color-space srgb --managed --append
cutcraft -p my-film.cut asset list --source-type image-sequence
cutcraft -p my-film.cut asset frames ASSET_ID --offset 0 --limit 100
```

也支持 `--pattern-style hash` 的 `####`、有序 `--files` 清单、显式起止编号和 `--missing error|hold|skip`。模板及缺帧策略只在导入时执行；之后按照项目中的固定清单采样。旧 0.3/0.4 项目导入图片序列会报 `PROJECT_UPGRADE_REQUIRED`；目前需新建项目，尚无原地升级命令。详见 [使用指南](docs/image-sequences.md)。

```sh
cutcraft validate spec/examples/animated.cutcraft.json
cutcraft validate spec/examples/batch.operation.json --kind operation --level schema
cutcraft -p my-film.cut validate --level media
```

- Schema：离线 Draft 2020-12 校验，含 UUIDv7、类型与格式约束。
- Semantic：引用、全局 ID、序列无环、轨道匹配、重叠、时间、字幕、关键帧、LinkGroup 双向引用及已知转场 handle。
- Media：检查本地素材、SHA-256、实际 stream、源范围及当前生成器／效果可用性。具体后端还会在渲染前拒绝不支持的视听行为。

旧版 0.3 项目的基础 FFmpeg 后端支持 CFR 视频、图片、硬切、多视频轨普通合成、视频 fit 变速／反放、透明度、颜色／文本生成器、字幕、音频裁剪／混音／增益、mute 和 solo。视频边界要求对齐帧网格，音频边界要求对齐采样网格。

支持嵌套序列、调整层、变换／裁剪、调色、RGB 曲线、1D/3D LUT、属性／效果动画、内置转场、精确变速、loop/hold 和带有效时间戳的 VFR。音频支持显式重采样、gain/pan 动画与线性淡化；光流、保音高、自定义插件和非普通混合模式仍明确报错。

`preview` 只导出当前 revision 的低分辨率 MP4，路径通过 JSON 的 `video` / `output` 返回，文件位于项目的 `cache/` 下。最长边限制为 640 像素，最高 24 fps；较小素材不放大，较低帧率保持原值，尺寸取偶数以兼容 H.264。素材解码、特效合成和嵌套序列均使用缩小的画布，编码采用 ultrafast / CRF 28，音频采样率最高 24 kHz。重新运行会重新生成整段序列预览；不再生成 HTML，也不再提供 `--open`。项目格式和正式 `render` 导出参数不变。默认系统字体依赖机器环境；需要固定字幕字体时使用 `style.fontAssetId`。输出像素的可复现性还取决于 FFmpeg、字体和编码配置版本。

## 开发

核心模块：`time` 精确时间；`schema` 内嵌合同；`project` 领域校验与采样；`edit` 纯事务命令；`store` 持久化；`media` 素材适配；`animation` 通用动画与精确映射；`timeline` CLI 原子编辑编译；`frame_render` 逐帧参考渲染及低分辨率预览；`render` 旧版媒体后端及导出入口。CLI 在 `src/main.rs`。

`tests/domain.rs` 验证领域不变量、回滚、撤销与恢复。`tests/media.rs` 使用合成素材走完整 CLI，并解码输出视频检查切点像素。没有 FFmpeg 时媒体测试报告跳过；CI 设置 `CUTCRAFT_REQUIRE_FFMPEG=1` 强制执行。

```sh
CUTCRAFT_REQUIRE_FFMPEG=1 cargo test --locked
cargo fmt --check
```

使用已提交的 `Cargo.lock` 构建。锁文件固定了适配 Rust 1.86 的 IDNA/ICU 依赖版本；升级依赖时应复测 MSRV。

项目使用 `cutcraft.json`，内含协议版本及创建/编辑应用版本。所有项目读写通过 `.cutcraft.lock` 的 OS 排他锁互斥；PID 仅用于诊断。旧 `project.json` 需显式迁移，当前 CLI 不自动改动旧项目。

CLI 0.1.0 支持视觉处理：偏移/缩放/旋转/裁剪、调色、RGB 曲线及 LUT/.cube。使用方式见 [视觉效果操作](docs/visual-effects.md)。

CLI 0.1.0 提供 `item animate`、`item speed`、`item time-map`、`transition`。完整示例和渲染边界见 [动画与时间映射指南](docs/animation-and-retime.md)。
