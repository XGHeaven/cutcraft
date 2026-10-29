# CLI 预览与画面检查

`preview` 不修改工程、revision、编辑者信息或历史。命令读取固定快照后释放编辑锁，输出 JSON 标明该 revision；已打开项目仍通过同一个 ApplicationHost 执行。

```sh
# 保留原有整段低分辨率 MP4
cutcraft -p film.cut preview
# 时间精确到有理秒；PNG 保留透明度，使用 sRGB
cutcraft -p film.cut preview --at 1001/30000
# 仅生成所需区间，并保留声音
cutcraft -p film.cut preview --start 5/2 --duration 3 --scale 0.5
# 从指定位置到结尾，或从零开始指定时长
cutcraft -p film.cut preview --start 5
cutcraft -p film.cut preview --duration 2
```

所有模式支持 `--sequence SEQUENCE_ID` 和 `--progress human|json|none|auto`。
`--at` 不能与 `--start` / `--duration` 组合；单帧要求 `0 <= at < 序列时长`。
区间按左闭右开 `[start,start+duration)` 处理，start 默认 0，duration 默认剩余时长。
负时间、零/负时长和超出序列边界均明确拒绝，不静默截断。

`--scale` 是相对于序列尺寸的有限比例 `(0,1]`，尺寸向下取偶数（最小 2），并遵守网页预览相同的分辨率上限。
单帧和 MP4 省略 scale 时最长边不超过 640 像素；MP4 帧率最高 24fps，音频最高 24kHz。缩略图总览和边界图的默认尺寸见下文。
单独传 `--scale` 会通过交互预览引擎生成整段 MP4。
新增选项支持公开 0.1.0 与内部 0.4/0.5；内部 0.3 保留无选项的原有整段预览，不支持新增模式。

单帧复用网页的 PNG 合成及缓存，JSON 的 `output` / `image` 是文件路径。
区间复用网页的有界解码缓存、同一时间线求值器和音频混合，输出 `output` / `video`，以及
`startTime`、请求的 `duration`、`frames`、`audioSamples`、尺寸、帧率和音频采样率。
区间视频第 n 帧在 `start+n/fps` 采样，不把起点舍入到全局帧网格；音频第 n 个样本在 `start+n/sampleRate` 采样。
样本数分别为 `ceil(duration*fps)` 和 `ceil(duration*sampleRate)`；编码帧/样本的覆盖终点可能稍晚于请求终点，但不会在请求终点及之后采样。AAC 解码还可能含编码器尾部填充。

PNG 位于 `cache/interactive-v1/`，区间 MP4 位于 `cache/preview-ranges-v1/`；这些都是可重建缓存。
区间每次重新合成/编码，但复用有界源解码缓存，不会先渲染整条序列再裁剪，也不创建临时工程。
整段与区间 MP4 采用[流式导出](streaming-export.md)：音视频生产与编码并发，不落盘整条合成视频和 PCM，源解码缓存及最终 MP4 暂存保留。
只有成功编码、同步和原子发布后才报告完成；失败保留同路径的既有结果。
实现为 CPU 参考路径，不保证实时；长区间的未压缩合成中间文件仍会占用临时磁盘。

## 缩略图总览与边界帧

```sh
cutcraft -p film.cut preview --sheet --count 12 --columns 4
cutcraft -p film.cut preview --sheet --interval 10
cutcraft -p film.cut preview --sheet --start 10 --duration 5
cutcraft -p film.cut preview --boundaries
cutcraft -p film.cut preview --boundaries --at 5/2
```

`--sheet` 在选定半开区间内按 `start+i*duration/count` 等间隔采样，默认 12 张、4 列；count 为 1–128，columns 为 1–16，仅此模式可用。时间不舍入到帧网格。输出为带序号和精确秒数标注的 PNG 网格。

`--interval N` 改为从 start 开始每 N 秒采样一张，采样时间为 `start+i*N < start+duration`，张数为 `ceil(duration/N)`。支持十进制和有理秒（如 `0.5` 或 `1/2`），必须大于零，与 `--count` 互斥且仅用于 `--sheet`。不包含区间终点；间隔大于或等于区间时长时只取起点一张。不指定 count/interval 时仍默认 12 张。超过 128 张明确报错，请增大间隔或缩小区间，不静默截断。

`--boundaries` 默认收集所选序列中可见视频/字幕轨上启用 Item 的起止点，遵守轨道 enabled/solo，合并同一时间的边界。包含媒体、生成器、嵌套和调整层，不递归收集子序列内部切点。输出每行左侧 before、右侧 after；`--at` 可改为检查一个指定时间点（允许 0 和序列终点）。也可用 start/duration 筛选边界，筛选包含区间两端。最多 64 个边界，超出时请缩小区间，不静默截断。

边界采样使用**工程帧率**而非预览 MP4 的 24fps 上限：边界 b 的 after 帧号为 `ceil(b*fps)`，before 为该帧号减一，时间为 `frameIndex/fps`。因此它检查输出帧网格中严格早于边界和首个不早于边界的画面，非帧对齐边界不使用 epsilon；不足一帧的片段可能没有输出采样点。序列起点之前、终点及之后不存在的帧显示 N/A，JSON 中 time/frameIndex 为 null。检查的是完整合成结果，转场处会保留实际混合效果；不会隔离被遮挡片段或自动判断剪辑质量。

两种模式互斥，--sheet 不接受 --at；边界模式的 --at 不与 start/duration 组合。默认缩略图最长边 320（不放大小画布），可用 --scale 覆盖。结果包含 output/image、revision、sequenceId、cells（精确 time、格子位置及画面尺寸），边界模式另有 boundaries（边界时间和相关 Item/轨道 ID）、frameRate 和每格的 side/frameIndex。空序列、无匹配边界、非法参数及超出拼图限制（任一边 32768 或总计 1600 万像素）明确报错。

输出位于 `cache/preview-sheets-v1/`，每次复用同一个 PreviewEngine 合成所需帧，再原子发布一张 PNG。格式支持与单帧预览相同：公开 0.1.0 和内部 0.4/0.5；无需改变工程协议或规范修订号。

MP4 预览可通过 `--video-encoder` 和 `--video-bitrate` 配置编码器与目标码率，见[编码配置](streaming-export.md#选择编码器)。完整预览和区间预览共用配置，缓存按配置区分；PNG/拼图/边界帧拒绝非默认视频编码选项。网页 HLS 播放仍用 libx264。
