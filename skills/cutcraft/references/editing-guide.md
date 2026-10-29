# Editing guide

Use this reference for project/media organization, sequences/tracks/cuts, audio, text/images/backgrounds, nesting/adjustments, domain commands/history, and validation/delivery. Motion, speed, transitions, color and LUT details are in [motion-and-effects.md](motion-and-effects.md).

Commands below use placeholders such as PROJECT, SEQUENCE_ID, TRACK_ID and ITEM_ID. Replace them with actual paths or IDs from CLI JSON output. JSON fragments describe fields to insert into a complete entity unless explicitly labeled as a complete command. `*.put` replaces an entity; retain existing fields and children when updating it.

## Projects and media

- `init DIRECTORY --name NAME` creates a project; `inspect` returns its current snapshot. Project snapshots include specVersion and application version history.
- `probe FILE` inspects media without importing it. `import FILE` registers it; `--managed` copies it into assets/, otherwise the project stores a relative file URI. `--color-space` supplies an explicit supported source declaration when needed.
- `--append` creates items/tracks and an A/V LinkGroup for one imported asset. It does not concatenate repeated imports into one track. Images append as five-second clips; embedded cover pictures are excluded from automatic video append.
- Assets are reusable: multiple items can reference one asset/stream with independent source ranges, effects and timeline positions. Inspect actual stream IDs rather than assuming every asset has stream_v0/stream_a1.
- For relinking, use `asset.put` with the complete updated asset and correct stream metadata/fingerprint, then validate media. Editing only a file URI while retaining an unrelated fingerprint will fail verification.
- `project.update` changes allowed top-level properties, such as name or rootSequenceId. It is not an arbitrary JSON Patch endpoint.
- `asset.remove` removes a project record; it is not a command to delete the original media from disk. References must be resolved or explicitly cascaded. Cascade can remove timeline items and related links/transitions; inspect the affected scope first.

### Cache maintenance

```sh
cutcraft -p PROJECT cache status
cutcraft -p PROJECT cache clean --dry-run
cutcraft -p PROJECT cache clean
```

The shared manager tracks PNG/thumbnail/waveform/text, decode windows, preview MP4/sheets, browser playback and temporary work. Default disk target: 1 GiB per project; decode/playback each 512 MiB; frames, text and sheets each 64 MiB/256 entries. Active work and current asset references are protected, so usage can temporarily exceed the target. `clean` removes only recognized reclaimable entries and preserves project/history, original media, unknown files, symlinks and stable coordination locks. `--dry-run` reports the prospective removal without deleting data. Cache hits refresh recency; keys and validity remain feature-specific. Session-local memory buffers have independent limits. Cache cleanup does not change specVersion or revision.

### Search and locate

```sh
cutcraft -p PROJECT asset search '喜字'
cutcraft -p PROJECT asset search --name 'IMG_' --path '云南'
cutcraft -p PROJECT asset list --kind image --tag wedding
cutcraft -p PROJECT asset list --unused
cutcraft -p PROJECT asset info ASSET_ID
cutcraft -p PROJECT asset usages ASSET_ID
cutcraft -p PROJECT item list --asset ASSET_ID
cutcraft -p PROJECT item search 'title text'
cutcraft -p PROJECT item list --sequence SEQUENCE_ID --track TRACK_ID
cutcraft -p PROJECT item list --sequence SEQUENCE_ID --at 3/2
cutcraft -p PROJECT item info ITEM_ID
cutcraft -p PROJECT sequence list
cutcraft -p PROJECT track list --sequence SEQUENCE_ID
cutcraft -p PROJECT sequence usages CHILD_SEQUENCE_ID
```

- `search` aliases `list` for assets/items. Text filters use case-insensitive literal substrings; multiple filters combine with AND. Asset `--kind` is exact; `--tag` matches a whole tag ignoring case. Name collisions return all matches. Asset search includes ID/name/URI/decoded local path/tags; item search includes ID/name/tags/text and directly referenced asset ID/name/path or nested sequence name.
- Results provide revision, count and named arrays (`assets`, `items`, `usages`, `sequences`, `tracks`). `info` returns the complete entity plus its IDs/context. ID arguments/filters require exact IDs and reject missing entities. Queries do not edit snapshots/history or decode media; missing files are still searchable, and availability is recorded state rather than a live filesystem check.
- Usage rows include sequenceId/name, trackId/name, itemId, timelineRange, sourceRange and reference roles. Media, built-in LUT and caption/text-generator font references are counted, including disabled items/effects and non-root sequences. usageCount counts distinct referring items. `--unused` means no known built-in references; opaque plugin parameters/extensions are not analyzed.
- Query commands default to all sequences. Timeline times are local to the returned sequence; `--at` requires `--sequence` and matches `[start,end)`, including disabled items. Use sample for evaluated layers, transitions and source-time mapping. Editing defaults to root, so explicitly pass the returned sequence ID for non-root items.
- `sequence usages` lists direct parent items embedding a child sequence. Follow parent sequence IDs to locate higher-level uses; do not add times to infer root positions through trims, retiming or loops.

Complete command to rename a project:

```json
{"type":"project.update","changes":{"name":"Travel highlights"}}
```

Submit with `cutcraft -p PROJECT command command.json --base-revision REVISION`.

## Sequences, tracks and cuts

```sh
cutcraft -p PROJECT sequence add --name 'Portrait edit' --width 1080 --height 1920 --fps 30000/1001
cutcraft -p PROJECT track add --sequence SEQUENCE_ID --kind video --name 'V1'
cutcraft -p PROJECT track add --sequence SEQUENCE_ID --kind audio --name 'Music' --overlap allow
cutcraft -p PROJECT item move ITEM_ID --sequence SEQUENCE_ID --track TRACK_ID --start 3/2
cutcraft -p PROJECT item trim ITEM_ID --start 0 --duration 3 --source-start 1 --source-duration 3
cutcraft -p PROJECT item split ITEM_ID --at 3/2
cutcraft -p PROJECT item remove ITEM_ID --ripple remove-gap
```

Commands default to the root sequence unless `--sequence` is supplied. Creating a sequence does not switch the root. To render another sequence, pass its ID to render/preview; to make it the default, update rootSequenceId explicitly.

The sequence format controls resolution, output frame rate, audio sample rate and channel layout. `sequence add --duration D` sets a fixed duration; without it, duration is derived from enabled items and markers. Update an existing format by cloning the sequence and submitting `sequence.put`, preserving its tracks/links/transitions/markers. A fixed duration can truncate the timeline or leave background/silence after items end.

Tracks organize video, audio and subtitles. Later video tracks composite over earlier video tracks; subtitle tracks are rendered after video. The normal blend mode is implemented. `--overlap forbid` is the default; use allow when intentional overlapping audio or layers need it. A transition does not require physically overlapping timeline ranges: it joins adjacent items and requests source handles.

Track name, enabled, locked, muted, solo, overlapPolicy and items are edited through full `track.put`; there are no dedicated track mute/solo/rename commands. `track.reorder` takes every track ID exactly once. `solo` filters enabled tracks of the same kind; audio mute suppresses sound. A data track may be stored, but is not a general data-to-video renderer.

For cuts, timelineRange locates the item in the sequence; sourceRange selects the reusable input interval. Static timed trim needs both complete ranges. Images and generators do not use sourceRange. Animated/remapped trimming has additional preservation rules in the motion reference.

Ripple semantics are operation-specific:

| Operation | Relevant policy |
| --- | --- |
| Move + none | Change the item position; reject forbidden overlap |
| Move + insert | Shift destination followers by the moved item's duration |
| Trim/speed/time-map + insert or remove-gap | Shift affected followers by the change in the end/duration |
| Move + remove-gap | Close the old interval before placing the moved item |
| Remove + remove-gap | Close the removed interval |
| Move/trim + overwrite | Keep representable slices of competing items; reject unsupported linked/transition cases |

Followers' link groups move together once; sequence markers do not move with ripple. Linked move/trim/split/remove may affect A/V partners. Use `link.put`/`link.remove` to set membership explicitly, preserving group IDs and item references. Splitting creates new item/effect IDs and repairs supported links/transition endpoints; read the resulting snapshot for new IDs.

## Audio

Import an audio file or select an audio stream from a video asset, then place it on an audio track with item add. Separate video and audio items can share an A/V link group; their render settings remain independent.

Set static sound controls by replacing the full item with this audio fragment merged in:

```json
{
  "audio": {
    "gainDb": -6,
    "pan": 0,
    "muted": false,
    "fadeIn": {"num": 1, "den": 4},
    "fadeOut": {"num": 1, "den": 2}
  }
}
```

Gain is in dB. Stereo pan runs from -1 (left) through 0 (center) to 1 (right). Legacy fadeIn/fadeOut fields create linear amplitude fades; gainDb/pan can instead use registered keyframe animation for more control. Avoid applying both fades and a gain envelope unless the combined attenuation is intended. Legacy nonzero fade fields cannot currently be sliced faithfully; use explicit replacement/animation when needed.

Tracks mix without automatic normalization or loudness mastering. Check the resulting audio rather than assuming overlaps cannot clip. Mono/stereo output is supported; surround layouts are not. Video transitions do not automatically crossfade linked audio.

`builtin.silence` 1.0.0 is an audio generator with empty parameters `{}` and a timelineRange. Audio adjustment items are unsupported. Speed changes require the explicit policy described in the motion reference; preserve-pitch is not implemented.

## Text, images and backgrounds

For a blurred backdrop, reuse visual media on a lower video track, scale it to cover the canvas, and blur only that item while keeping the foreground sharp. Follow the [blurred-background recipe](motion-and-effects.md#模糊背景最佳实践) for track order, exact source-time synchronization and avoiding duplicate audio.

```sh
cutcraft -p PROJECT track add --kind subtitle --name 'Captions'
cutcraft -p PROJECT item caption --track SUBTITLE_TRACK_ID --start 1 --duration 2 --text 'Arriving in Yunnan'
cutcraft -p PROJECT item color --track VIDEO_TRACK_ID --start 0 --duration 3 --color '#203040'
```

Both `item caption` and `item text` accept `--text` or `--text-file` (UTF-8 with explicit newlines), `--font-asset` or `--font-family` (mutually exclusive), `--font-size`, `--color '#RRGGBBAA'`, `--align` and `--position`. Use `item text` on a video track for editable titles; use `item caption` on a subtitle track. File text is stored directly in the project, not linked. Use `--font-family 'PingFang SC'` for installed macOS Chinese fonts, or an exact PostScript face name such as `PingFangSC-Semibold`. Cutcraft discovers system fonts and selects the correct TTC face. Unknown names and missing glyphs fail explicitly; never silently substitute another font. Pinned `fontAssetId` takes precedence and still uses face zero. System fonts are not bundled, and installing new fonts requires a fresh CLI process or editor-service restart. Automatic wrapping, text boxes and rich text are not implemented.

Caption items carry text and timelineRange. Customize the full item's style with supported fontSizePixels, color, align (left/center/right), position (top/center/bottom), and fontAssetId. Color uses numeric RGBA in [0,1]. Caption rendering burns text into the video; this is not automatic transcription, translation, speech alignment, or an SRT import/export facility. Word timing metadata can be stored, but does not imply animated karaoke rendering.

For a title on a video track, use an item.put containing a generator item with these fields (plus id and timelineRange):

```json
{
  "type": "generator",
  "generatorType": "builtin.text",
  "version": "1.0.0",
  "parameters": {
    "text": "A weekend away",
    "fontSizePixels": 48,
    "position": "center",
    "align": "center",
    "color": {"r": 1, "g": 1, "b": 1, "a": 1}
  }
}
```

Text generators support video transforms/effects/animation. The fallback system font depends on the machine; fontAssetId points to a registered font asset for repeatability. Normal media import does not register font files: use asset.put with kind=font, a resolvable URI and streams=[] after inspecting the asset schema. Explicit fontFamily lookup is supported as described above; the renderer rejects backgroundColor and maxLines style features. Do not assume every stored caption style has a rendering implementation.

Images are media items on video tracks with timelineRange but no sourceRange. Use item transform for position, anchor, scale, crop and rotation; animate those properties for a moving still. Position/anchor are normalized, scale is multiplicative, crop edges are fractions, and rotation is degrees. Negative scales mirror; zero scale hides the geometry. A transparent PNG can overlay lower tracks. Select an appropriate supported color declaration for the image.

`builtin.color` produces a solid clip with optional alpha. Sequence background supplies the base canvas outside item coverage; change it through sequence.put. Use a color item when the background should have its own timeline, effects or animation.

## Nested sequences and adjustment layers

To reuse an edited segment, create an item with type=sequence, sequenceId identifying the child, and both timelineRange/sourceRange; add it using item.put. A sequence reference is not an imported video file. The child must exist, the source range must fit its duration, and the reference graph must have no cycles.

Nested picture and sound are placed as separate items on video/audio tracks. Link them if subsequent edits should move together. The child renders at its own mapped source time; the parent then applies its own transforms/effects/opacity. This supports picture-in-picture, reused compositions and retiming a composition with its internal animation. It does not create a self-contained external subproject package.

A video adjustment item has type=adjustment, timelineRange and an ordered effects array; no sourceRange or media asset is needed. Place it above the layers it should affect. While active, it processes the composited image below it; use opacity to control its strength. For example, an adjustment layer with color-grade can grade a multi-layer scene together. It is not an audio bus or an arbitrary masking system.

## Domain commands and history

All mutations use CLI convenience commands or `command`/`apply`; do not write the snapshot/history directly. The domain interface exposes:

| Scope | Commands |
| --- | --- |
| Project | project.update |
| Media/sequence aggregates | asset.put, asset.remove, sequence.put, sequence.remove |
| Tracks | track.put, track.remove, track.reorder |
| Items | item.put, item.remove, item.move, item.trim, item.split |
| Transitions | transition.put, transition.remove |
| Links | link.put, link.remove |
| Review markers | marker.put, marker.remove |
| Atomic grouping | batch |

A batch contains non-batch commands; nested batches are rejected. Whole-project validation happens after the group, allowing mutually dependent additions in one transaction. Locks remain enforced. Schema definitions for all command payloads are available from `cutcraft schema operation`.

```sh
cutcraft -p PROJECT command changes.json --base-revision REVISION --dry-run
cutcraft -p PROJECT command changes.json --base-revision REVISION
cutcraft -p PROJECT history
cutcraft -p PROJECT undo
cutcraft -p PROJECT marker --name 'Check this cut' --at 5/2 --duration 1/2
```

`command` adds an operation envelope; `apply` expects the complete envelope. Successful batches increase revision once. Semantically rejected operations may appear in history without changing the project snapshot. History records inverse commands and hashes; it is not a branch manager or a general arbitrary-revision restore API. Repeated undo toggles the latest operation/inverse rather than traversing an undo stack.

Markers describe a point or interval for review; they do not add visible text to the output. In auto-duration sequences, a late marker can extend duration. Exported results identify the captured revision, so edits made during a render do not change that render halfway through.

## Validation and delivery

```sh
cutcraft -p PROJECT validate --level schema
cutcraft -p PROJECT validate --level semantic
cutcraft -p PROJECT validate --level media
cutcraft -p PROJECT sample --sequence SEQUENCE_ID --at 1001/30000
cutcraft -p PROJECT preview --sequence SEQUENCE_ID --progress json
cutcraft -p PROJECT render --sequence SEQUENCE_ID --output output/final.mp4 --progress json
```

Schema checks shapes/types; semantic checks IDs, references, times, overlap, links and animations; media checks local files, fingerprints, streams, source bounds and relevant backend availability. Runtime sampled constraints may still reject a render, so media validation alone does not establish a successful export.

`sample` reports active items, evaluated properties, exact source positions and transition roles/progress without decoding images. No-option `preview` exports a full low-resolution MP4 under project-root `cache/`, with a 640-pixel long edge and at most 24fps. Use `--at` for PNG, `--start/--duration` for interval MP4, `--sheet` for a thumbnail overview, or `--boundaries` for paired frames around cuts. All modes capture a fixed revision without changing the project. See the recipes below; use `open` for browser playback.

The export is H.264/AAC MP4 with even dimensions, square pixels and Rec.709 output. Use `--video-encoder libx264|h264_videotoolbox|h264_nvenc|h264_qsv` on render or MP4 preview; default is libx264. Optional `--video-bitrate 8000000` sets target bits/s (integer 1–1000000000); hardware defaults are 8 Mbps for render and 2 Mbps for preview. Hardware choices require the matching FFmpeg build, driver and device and fail without software fallback. These runtime options do not change the project or enable GPU compositing/decoding; browser HLS still uses libx264. Read the result `encoding` metadata. PNG/sheet/boundary previews reject non-default video encoding settings. There are no flags for arbitrary codecs or CRF; resolution and audio layout belong in the sequence. Existing destination files require --overwrite. Failures preserve the previous output, and source-file overwrite is rejected.

Do not claim native project exchange with other NLEs, ASR, remote URL media resolution or automatic real-time/GPU processing. Use installed capabilities for the current boundary. Return the project and actual exported artifact, noting any requested feature that could not be realized.

Render/preview progress is JSONL on stderr with `--progress json`; stdout remains the final result JSON. Events identify validation, video/audio processing, decoding, encoding and finalization. `estimatedRemainingPhaseSeconds` estimates only the current phase; null means unknown. Only `status=completed` after atomic publication indicates completion. Errors are the existing error JSON after any progress events. `auto` is human-readable for terminals and silent otherwise.

Check rendered subtitles over a visible background and inspect glyph edges. Schema/media validation does not establish visual correctness; frame sampling and signal metrics do not replace full playback review. Report the review actually performed.

## 本机共同编辑

使用 `cutcraft open <项目目录>` 注册项目并打开网页，`--no-browser` 只返回地址。默认 loopback 地址免登录，可重复打开；`--host localhost` / `--host ::1` 同样免登录，`--host 0.0.0.0` 等非 loopback 地址启用一次性启动码和会话认证。改变运行中服务的监听地址前须关闭所有项目；`cutcraft close <项目目录>` 关闭项目，最后一个项目关闭后退出后台进程。仅已打开项目的 CLI 命令自动转发，其他项目独立执行。导入/导出忙碌时 close 拒绝，须等待或取消任务。

网页与 CLI 共享 revision 和持久历史。`history undo` / `history redo` 为项目级多步撤销重做；原 `undo` 保留旧行为。发生 revision 冲突先刷新再重试；响应丢失或服务重启后不得自动换 operationId 重复执行未知结果。详见 [本机编辑器](../../../docs/local-editor.md)。

界面面向 PC 浏览器，面板随窗口调整，工具栏换行保留全部入口；时间线可在自身区域内横向滚动。网页默认以 0.5× 预览，可切换 1× / 0.25×；切换会暂停并保留位置，不修改工程或正式导出尺寸。项目管理、导入、导出、任务与片段详细属性在弹窗中，选中片段后点击 Properties 或双击打开。后端静帧/草稿/播放 API 接受连续 `scale`（有限数值，`0 < scale <= 1`）；省略时兼容 640px 上限，参见本机编辑器文档。

## 单帧与区间预览

`preview --at 3/2` 返回 PNG 的 `output` / `image` 路径。
`preview --start 2 --duration 3 --scale 0.5` 返回带音频 MP4 的 `output` / `video` 路径；两者复用网页预览引擎，支持 `--sequence`、`--progress`。
`--at` 不能和区间参数混用；区间左闭右开，不允许超出序列。时间用有理秒，scale 为 `(0,1]`。
新增模式支持公开 0.1.0 和内部 0.4/0.5；无选项 `preview` 保留原有整段 MP4 与旧版兼容。
结果只对应返回的 revision，不写快照或历史，输出是可重建 cache；单帧或抽帧检查不代表完整主观试听。


## 缩略图总览与边界帧检查

```sh
cutcraft -p PROJECT preview --sheet --count 12 --columns 4
cutcraft -p PROJECT preview --sheet --interval 10
cutcraft -p PROJECT preview --sheet --start 10 --duration 5
cutcraft -p PROJECT preview --boundaries
cutcraft -p PROJECT preview --boundaries --at 3/2
```

先用 `--sheet` 浏览整体内容，再用 `--boundaries` 检查切点。两者输出 `image` / `output` PNG，`cells` 给出每格精确 time 和图中坐标；默认缩略图最长边 320，可用 `--scale` 调整。总览默认 12 张、4 列，以 `start+i*duration/count` 在半开区间内均匀取样，不包含终点；count 最多 128，columns 最多 16。需要每 N 秒一张时用 `--interval N`，支持 `0.5` / `1/2` 等正有理秒，与 `--count` 互斥。它从 start 起按固定间隔采样，不包含区间终点；间隔大于或等于时长时只取起点，张数为 `ceil(duration/N)`。超过 128 张会拒绝，需增大间隔或缩小区间。

边界模式默认按时间合并可见视频/字幕轨启用 Item 的起止点（遵守 solo），每行左 before、右 after；也可用 `--at` 只检查指定点，或用 start/duration 筛选边界（两端包含）。最多 64 个边界，超出时缩小区间。`boundaries` 提供相关 Item/轨道 ID；不递归枚举嵌套内部切点，要检查子序列时指定 `--sequence`。

边界帧使用工程帧率：after 帧号为 `ceil(boundary*fps)`，before 减一，时间为帧号除以 fps。它们是实际输出帧网格上的两侧画面，不是对边界时间减浮点 epsilon。序列首尾缺少的一侧为 N/A，JSON time/frameIndex 为 null；非常短的片段可能没有输出帧。检查的是最终合成画面，包含实际转场、字幕和叠层；缩略图不能替代区间播放、试听，也不会自动证明不存在黑帧或剪辑错误。

`--sheet` / `--boundaries` 互斥，count/columns/interval 仅用于 sheet；sheet 不与 at 组合，at 不与 start/duration 组合。支持公开 0.1.0 和内部 0.4/0.5；内部 0.3 不支持，不改版本绕过。输出在可重建的 `cache/preview-sheets-v1/` 中，工程、历史和媒体保持不变。


## 环境诊断

运行 `cutcraft doctor` 检查当前 CLI 的 FFmpeg / ffprobe 环境，无需创建或打开工程。需要时用 `--timeout 20` 调整每项外部检查的超时（默认 10 秒，1–120）。它检查版本、必需编码器/滤镜、文件滤镜图参数，并在自动清理的临时目录测试文字 PNG、FFV1/PCM、H.264/AAC MP4 和探测/解码。

检查完成后 stdout 总是完整 JSON，`checks` 的 status 为 passed/failed/warning/skipped，包含具体缺失组件或错误 code、hint；任一 failed 时退出码为 1，不要因此丢弃 stdout 的报告。区分 MEDIA_TOOL_NOT_FOUND、MEDIA_TOOL_PERMISSION_DENIED、MEDIA_TOOL_TIMEOUT 和组件缺失，不要把所有错误当作“未安装”。该命令不安装工具或修改工程；发现问题后按报告处理。

该诊断不转发到已打开工程的服务进程。修改 PATH 后重启旧编辑器服务，不能用当前 CLI 的成功诊断证明旧服务环境已更新。doctor 只检查基础媒体路径和默认字体，具体素材、苹方字形、颜色声明仍需项目 `validate --level media` 及预览验证。

### Streaming MP4 output

`render` on the reference backend and full/interval MP4 previews stream video and audio to FFmpeg through separate loopback TCP inputs. CLI syntax and quality settings are unchanged; no `--stream` or `--jobs` is needed. Video/audio production and encoding overlap, but video frames are still composed sequentially. Source decode data and the final temporary MP4 remain; only the composed raw video/PCM spool is removed. `--progress json` phase events may interleave; wait for successful publication and final JSON, not a single phase reaching 100%. Failed/cancelled exports preserve existing output. The environment must allow local loopback connections.
