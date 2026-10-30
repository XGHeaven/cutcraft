# 流式导出

参考渲染器的 `render`、整段 MP4 预览和区间 MP4 预览会直接把合成数据送给 FFmpeg，不再把整条时间线先写成 `video.rgba` 和 `audio.f32`。默认编码质量、精确采样时间和工程协议保持不变；不需要添加 `--stream`。

```sh
cutcraft -p film.cut render --output output/final.mp4 --progress json
cutcraft -p film.cut preview
cutcraft -p film.cut preview --start 5/2 --duration 3
```

## 选择编码器

`render` 和 MP4 `preview` 支持运行时编码配置；不写入 `cutcraft.json`，不改变 Spec 版本。CLI 将配置随请求传给 ApplicationHost，已打开项目也使用本次指定的配置。

```sh
# macOS 硬件编码；码率单位 bits/s，这里是 8 Mbps。
cutcraft -p film.cut render --video-encoder h264_videotoolbox --video-bitrate 8000000
cutcraft -p film.cut preview --start 0 --duration 3 --video-encoder h264_videotoolbox
# NVIDIA 或 Intel 的对应选择（需本机 FFmpeg 构建、驱动和设备支持）
cutcraft -p film.cut render --video-encoder h264_nvenc
cutcraft -p film.cut render --video-encoder h264_qsv
```

| `--video-encoder` | 编码方式 | 未指定码率时 |
| --- | --- | --- |
| `libx264`（默认） | CPU 软件编码 | 导出 fast / CRF 18；预览 ultrafast / CRF 28 |
| `h264_videotoolbox` | Apple VideoToolbox | 导出目标 8 Mbps；预览目标 2 Mbps |
| `h264_nvenc` | NVIDIA NVENC | 同上；导出 p4，预览 p1 |
| `h264_qsv` | Intel Quick Sync | 同上；导出 medium，预览 veryfast |

`--video-bitrate` 接受 1–1000000000 的整数 bits/s；软件编码指定它后也改用目标码率模式，不再设置 CRF。目标码率不是严格恒定码率，输出实际码率可能更低。不同编码器的码率、画质与速度不能直接等同；高分辨率素材可按需求提高码率。

硬件编码只加速最终 H.264 编码；源解码、图层合成、效果与颜色转换仍使用 CPU。缺少编码器、驱动或设备时导出失败，保留 FFmpeg 原因，不自动重试软件编码；VideoToolbox 显式禁用软件回退。仍通过原子发布保护已有成片。

ApplicationHost 请求中的可选 `encoding` 对象形如 `{"video_encoder":"h264_videotoolbox","video_bitrate":8000000}`；省略对象/字段使用默认值，码率可为 null，未知编码器、未知字段和超范围码率拒绝。它属于运行时命令配置，不是工程或 Operation/Event 字段。

结果 JSON 的 `encoding` 返回 `videoEncoder`、有效 `videoBitrate`、`rateControl` 和 `crf`；MP4 预览缓存键包含编码配置。`capabilities` 列出的编码器是适配范围，不代表本机可用性；不带参数的 `doctor` 只检查默认软件链路；用 `doctor --video-encoder h264_videotoolbox`（或指定其他编码器）实测硬件编码与解码像素。

图片预览（`--at`、`--sheet`、`--boundaries`）拒绝非默认视频编码配置。网页交互 HLS 播放维持 `libx264`；本次不新增网页设置、自动选择编码器、硬件解码或 GPU 合成。

## 工作方式

```text
视频 worker：按时间戳顺序合成 RGBA16 帧 ──本机 TCP──┐
                                                   FFmpeg → 临时 MP4 → 原子发布
音频 worker：按样本时钟混合 float32 PCM ──本机 TCP──┘
```

视频与音频分别由一个生产线程处理，同时由一个 FFmpeg 进程编码。视频帧仍按顺序生成，本次没有新增多 worker 并行合成帧或 `--jobs`。两路输入独立写入，避免 FFmpeg 等待视频时，生产端因音频通道阻塞而无法继续提供视频。各 worker 使用独立的源文件游标和暂存子目录。

发送端保留当前视频帧或最多 4096 个采样帧的音频块；socket 写入受操作系统缓冲区限制，FFmpeg 消费变慢时，生产端会等待。不会为了排队而缓存整条输出。这个限制只针对输出传输，源解码、合成画布、效果处理和编码器自身仍需要内存或磁盘。

传输复用浏览器播放的本机 TCP 接收/写入与取消处理，只监听 `127.0.0.1` 的临时端口，不访问远程服务。运行环境需要允许本机 loopback 连接，FFmpeg 构建需要包含 TCP 协议。文件输入的旧版基础后端保留原来的渲染路径。

## FFmpeg CLI 为什么能接收流

`ffmpeg -i` 接受输入地址；它可以是普通文件，也可以是 `pipe:0`（标准输入）、TCP 等协议地址。程序不断写入字节，FFmpeg 就不断读取、解码/解释、编码。单路视频最简示例是：

```sh
# 生产程序输出恰好 640×360、每像素 3 字节的 RGB24 帧；日志另走 stderr。
producer | ffmpeg -f rawvideo -pixel_format rgb24 -video_size 640x360 \
  -framerate 30 -i pipe:0 -an -c:v libx264 -pix_fmt yuv420p output.mp4
```

`producer` 是示意用的生产程序，不是 Cutcraft 子命令。裸视频没有文件头，FFmpeg 无法自行知道宽高、像素布局和帧率，因此必须显式声明这些输入参数。TCP 的一次 write/read 不一定对应一整帧；FFmpeg 按尺寸和像素格式确定每帧需要多少字节。PCM 同样需要声明采样格式、采样率和声道数。

Cutcraft 要同时发送视频和音频，因此使用两个本机 TCP 输入。内部命令的核心结构如下；端口、分辨率、帧率和采样率由程序生成，实际命令还包含颜色转换、编码参数和进度参数：

```sh
ffmpeg \
  -f rawvideo -pixel_format rgba64le -video_size 1920x1080 -framerate 30 \
  -i tcp://127.0.0.1:VIDEO_PORT \
  -f f32le -ar 48000 -ac 2 -i tcp://127.0.0.1:AUDIO_PORT \
  -map 0:v:0 -map 1:a:0 -c:v libx264 -pix_fmt yuv420p -c:a aac staging.mp4
```

原始视频帧和 PCM 的时间戳来自帧率、采样率及输入顺序，不来自字节抵达的墙上时钟。视频仍在 `n/fps` 采样，区间预览则在 `start+n/fps` 采样；音频同理。生产速度快慢只影响导出耗时，不改变播放速度。离线导出不加 `-re` 实时限速，也不靠 `-shortest` 截掉尾部。关闭输入表示 EOF；FFmpeg 消费完剩余数据并完成编码后退出。

见 FFmpeg 官方的 [rawvideo 格式说明](https://www.ffmpeg.org/ffmpeg-formats.html#rawvideo)、[pipe 协议](https://www.ffmpeg.org/ffmpeg-protocols.html#pipe)和 [TCP 协议](https://www.ffmpeg.org/ffmpeg-protocols.html#tcp)。

## 仍然保留的数据与失败行为

- 源素材解码缓存仍保留。倒放、时间重映射和嵌套需要按素材时间访问数据；流式输送合成输出不等于把源读取也改成单向流。
- 最终编码结果仍先写临时 MP4，成功后同步并原子发布；不直接往已有成片上持续写。`--overwrite` 规则不变。
- 任一路生产失败或用户取消，都会停止两路输入并终止、回收 FFmpeg。编码器提前退出也会唤醒等待连接/写入的生产线程。传输错误保留底层媒体错误，不报告任务完成。
- stdout 仍只输出最终 JSON；`--progress json` 的 stderr JSONL 中 video、audio、encode 阶段可能交错，只有发布成功后才发 complete。各阶段的进度不能直接相加为总进度。

回归覆盖流式编码在生产结束前启动、与文件输入基线的实际解码像素/音频一致、源处理失败、缺失或提前退出的编码器、写入阻塞时的取消，以及取消后已有成片保持不变。减少了合成输出的落盘读写；具体速度提升取决于合成、源解码、磁盘和编码各自的耗时。
