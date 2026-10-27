# CLI 单帧与区间预览

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
省略时最长边不超过 640 像素；MP4 帧率最高 24fps，音频最高 24kHz。
单独传 `--scale` 会通过交互预览引擎生成整段 MP4。
新增选项支持公开 0.1.0 与内部 0.4/0.5；内部 0.3 保留无选项的原有整段预览，不支持新增模式。

单帧复用网页的 PNG 合成及缓存，JSON 的 `output` / `image` 是文件路径。
区间复用网页的有界解码缓存、同一时间线求值器和音频混合，输出 `output` / `video`，以及
`startTime`、请求的 `duration`、`frames`、`audioSamples`、尺寸、帧率和音频采样率。
区间视频第 n 帧在 `start+n/fps` 采样，不把起点舍入到全局帧网格；音频第 n 个样本在 `start+n/sampleRate` 采样。
样本数分别为 `ceil(duration*fps)` 和 `ceil(duration*sampleRate)`；编码帧/样本的覆盖终点可能稍晚于请求终点，但不会在请求终点及之后采样。AAC 解码还可能含编码器尾部填充。

PNG 位于 `cache/interactive-v1/`，区间 MP4 位于 `cache/preview-ranges-v1/`；这些都是可重建缓存。
区间每次重新合成/编码，但复用有界源解码缓存，不会先渲染整条序列再裁剪，也不创建临时工程。
只有成功编码、同步和原子发布后才报告完成；失败保留同路径的既有结果。
实现为 CPU 参考路径，不保证实时；长区间的未压缩合成中间文件仍会占用临时磁盘。
