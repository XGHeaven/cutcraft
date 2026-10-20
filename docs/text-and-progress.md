# 原生文字与渲染进度

## 文字

`item text` 在视频轨创建 `builtin.text`，`item caption` 在字幕轨创建 caption。
两者均支持：`--text` 或 `--text-file`、`--font-asset`、`--font-size`、`--color`、
`--align left|center|right`、`--position top|center|bottom`、`--start`、`--duration`、`--sequence`。
颜色使用 `#RRGGBB` 或 `#RRGGBBAA`，数值保存为协议的 RGBA 对象。

```sh
cutcraft -p demo.cut item text --track VIDEO_TRACK_ID --start 0 --duration 3 \
  --text '可编辑标题' --font-size 64 --color '#ffffffcc' --position center
cutcraft -p demo.cut item caption --track SUBTITLE_TRACK_ID --start 1 --duration 2 \
  --text-file captions.txt --font-size 28 --align left --font-asset FONT_ASSET_ID
```

`--text-file` 读取 UTF-8 内容并直接存入工程，不持续依赖这个文件；显式换行保留。
`--font-asset` 使用已注册的 `kind=font` 素材，缺少引用会被校验拒绝。字体文件可通过
`asset.put` 注册；普通媒体 `import` 不是字体导入命令。未指定字体时使用系统字体，跨机器不保证一致。
位置是预设锚点，对齐决定整个文字块的位置；更细的偏移/缩放可使用 Item 的 video.transform。
本轮没有引入新协议字段或改变项目版本。

渲染先生成白色字形覆盖率，再赋予 straight RGB 和用户设置的 alpha，确保未绘字区域透明，
避免 FFmpeg 在透明画布上混合半透明字体时重复计算 alpha。相同修复覆盖参考渲染器和旧版
需要 CPU 效果处理的文字路径。预览、嵌套和半透明文字均有像素级回归测试。

当前支持基础排版，不支持自动换行、文本框宽高、行距配置、富文本或逐字动画。
原协议中的 fontFamily、backgroundColor、maxLines 仍未提供渲染实现；不要把 PNG+SRT
当作工程内可编辑文字。复杂外部排版可以继续使用图片素材。

## 进度

```sh
cutcraft -p demo.cut preview --progress human
cutcraft -p demo.cut render --progress json --output output/final.mp4 2>progress.jsonl
```

`--progress auto|human|json|none` 默认 auto：stderr 是终端时输出 human，否则静默。
需要 Agent 持续监控时显式使用 json；重定向 stdout 不会改变进度输出位置。
Rust 库原有 render/preview 函数保持静默；`render_with_progress`、`preview_with_progress`
接受 `progress::Progress`，便于调用方选择 stderr 输出模式。

参考渲染阶段：validate → video → audio → encode → finalize → complete。
首次使用媒体时会在当前合成阶段内临时报告 probe/decode_video/decode_audio，完成后返回父阶段。
旧版基础后端主要报告 validate、encode、finalize；底层 FFmpeg 的进度不代表整个任务的进度。
total 无法确定时为 null，解码音频的 completed 单位为源媒体微秒，不是已经花费的时间。

JSONL 事件示例（数值仅用于说明）：

```json
{"type":"progress","phase":"video","status":"running","completed":24,"total":120,"unit":"frames","elapsedSeconds":3.0,"phaseElapsedSeconds":2.5,"estimatedRemainingPhaseSeconds":10.0}
```

视频计数按帧、音频计数按采样时间点（不是所有声道样本之和）。ETA 只估算当前阶段，
不承诺整段剩余时间；尚无有效样本或总量未知时为 null。事件通常最多每 0.5 秒更新一次，
阶段开始、计数完成及阶段切换立即报告。素材探测或单帧处理阻塞时不会虚构已完成量。

stdout 成功时仍是一个最终 JSON。错误时退出码非零，stderr 在此前进度之后输出原有
`{"ok":false,"error":...}` 错误对象；不会输出 completed。只有导出原子落盘成功后才报告 complete。
进度显示错误（例如 stderr 被关闭）不影响视频生成。现有目标文件的覆盖保护不变。

工程/媒体校验检查结构、引用和媒体约束，不证明视觉或听觉质量。成片仍应检查字幕叠加、
转场边界、字形边缘与完整视听体验；“预览与正式导出相似”不能单独证明两者正确。
