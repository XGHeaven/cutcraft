# Motion, time maps, transitions and effects

These commands target CLI 0.1.0. Check the installed `capabilities`, relevant `schema`, and `--help` when version support differs. `PROJECT`, `ITEM`, `FROM`, `TO`, `EFFECT_ID`, and `LUT_ASSET_ID` below are placeholders for actual paths/IDs.

## Property animation

```sh
cutcraft -p PROJECT item animate ITEM video.transform.rotationDegrees --file rotation.json
```

`rotation.json`:

```json
{
  "$animation": {
    "version": "1.0.0",
    "valueType": "number",
    "timeReference": "owner",
    "keyframes": [
      {"offset": {"num": 0, "den": 1}, "value": 0},
      {"offset": {"num": 2, "den": 1}, "value": 360}
    ]
  }
}
```

Offsets are strictly increasing, nonnegative and within item duration. An endpoint may equal duration. The last key must omit easing. Before/after the curve, values hold at the endpoints. Number/vector2 segments default to linear; outgoing `easing` belongs on the first key of a segment:

- `{"type":"hold"}` keeps the value until the next key, then jumps.
- `{"type":"cubic-bezier","x1":0.42,"y1":0,"x2":0.58,"y2":1}` eases the progress; controls lie in [0,1], with x1 <= x2.
- Rotation is unwrapped: 0→720 means two turns, not zero rotation.

Registered properties are `video.opacity`, `video.transform.position/anchor/scale` (whole vector2 or individual `.x/.y` number), `video.transform.rotationDegrees`, `video.transform.crop.left/right/top/bottom`, and `audio.gainDb/pan`. Whole-vector key values are static `{"x":...,"y":...}`. Replace a whole-vector animation before changing one component. Opaque plugin JSON is not interpreted as animation.

Item/effect animations follow item-local output time. A parent's reverse playback changes the sampled child sequence time, not the parent's own position/rotation clock.

## 录屏区域放大最佳实践

录屏中的局部放大、平移和恢复全景，组合现有 `item transform` 与属性动画完成，作为剪辑最佳实践使用，无需专用内置功能。直接变换录屏的视觉 Item，保持素材、时间映射和音频不变。

- **固定区域放大**：将目标区域中心设为 `anchor`，用 `position` 决定它在画布中的位置，再等比增大 scale。anchor 是裁剪后源矩形的归一化坐标，position 是画布归一化坐标；scale 相对 contain 尺寸。先检查已有 crop、旋转和动画，再确定参数。
- **动态聚焦**：使用上文的 owner 时钟属性动画，同时调整 `video.transform.scale` 和 `video.transform.position`，安排“全景 → 放大 → 停留/平移 → 恢复”。时间偏移相对 Item 起点，不是序列时间；两条曲线使用相同的关键时刻和缓动，停留段使用相同端点值。恢复时还原编辑前的构图。
- **边界与清晰度**：目标靠近边缘时调整放大倍率或目标位置，避免露出透明边缘；预览整个运动区间，检查文字清晰度和构图。放大不会增加录屏本身的细节。已有模糊背景时只动画前景，背景继续铺满画布。

例如，将裁剪后画面 `(0.65, 0.45)` 处放到画布中央，静态放大两倍：

```sh
cutcraft -p PROJECT item transform ITEM --anchor-x 0.65 --anchor-y 0.45 --x 0.5 --y 0.5 --scale-x 2 --scale-y 2
```

动态示例：输入和画布宽高比相同、无裁剪/旋转、anchor 始终为 `(0.5, 0.5)`、原构图为居中 scale=1 时，可用以下两条 vector2 曲线聚焦同一点。按上文 Property animation 格式分别保存为 `zoom.json` 和 `pan.json`；offset 用有理秒，Item 时长至少为 3 秒：

| offset | scale | position |
| --- | --- | --- |
| 0/1 | `{"x":1,"y":1}` | `{"x":0.5,"y":0.5}` |
| 1/2 | `{"x":2,"y":2}` | `{"x":0.2,"y":0.6}` |
| 5/2 | `{"x":2,"y":2}` | `{"x":0.2,"y":0.6}` |
| 3/1 | `{"x":1,"y":1}` | `{"x":0.5,"y":0.5}` |

此示例从原居中构图开始，不要先应用上面的静态 anchor 设置；不同宽高比或已有变换需按实际 contain 尺寸重新计算位置。

```sh
cutcraft -p PROJECT item animate ITEM video.transform.scale --file zoom.json
cutcraft -p PROJECT item animate ITEM video.transform.position --file pan.json
```

修改已有动画时保留区间外的关键帧与构图；需要同时发布多条曲线时，通过完整 `item.put` 或 batch 原子提交。手动设置聚焦点不代表自动识别鼠标、点击或跟踪目标。

## Speed and exact source mapping

```sh
cutcraft -p PROJECT item speed ITEM --rate 2 --animations stretch --ripple remove-gap
cutcraft -p PROJECT item speed ITEM --rate -0.5 --animations keep-local
cutcraft -p PROJECT item time-map ITEM --file mapping.json
```

Speed fixes sourceRange and computes the new timeline duration, using fit/reverse rather than storing another rate field. Replacing remap needs `--replace-remap`. Zero rate needs an explicit duration/time map. `--animations keep-local` is the default; out-of-bounds keys reject the transaction. `stretch` scales property key offsets with the new duration. Ripple `none` leaves followers in place; `insert/remove-gap` shifts affected followers by the duration delta.

Example `mapping.json` (accelerate, freeze, reverse; item duration becomes 5 seconds):

```json
{
  "$animation": {
    "version": "1.0.0",
    "valueType": "time",
    "timeReference": "owner",
    "keyframes": [
      {
        "offset": {"num": 0, "den": 1},
        "value": {"num": 0, "den": 1},
        "easing": {
          "type": "cubic-hermite",
          "outRate": {"num": 1, "den": 2},
          "inRate": {"num": 3, "den": 2}
        }
      },
      {"offset": {"num": 2, "den": 1}, "value": {"num": 2, "den": 1}},
      {"offset": {"num": 3, "den": 1}, "value": {"num": 2, "den": 1}},
      {"offset": {"num": 5, "den": 1}, "value": {"num": 0, "den": 1}}
    ]
  }
}
```

`value` is relative to sourceRange.start; the source window must contain all values and Hermite control values. Time maps use exact rational linear/Hermite evaluation, not accumulated floating point speed. Equal positions freeze; decreasing positions reverse. Do not add `reverse` to remap, use numeric floats for source time, or use hold/Bezier easing for time values. Jump cuts use separate items. Final offset sets duration; first offset must be zero.

Default frame-hold repeats/drops frames according to mapped PTS. To blend frames, preserve the full item and set `playback.videoSampling` to `{"method":"frame-blend"}` through item.put. Optical-flow has a schema interface but no installed processor.

For linked A/V speed/time-map commands, add `--audio-policy mute` or `--audio-policy resample` when audio is no longer +1 speed. The convenience command requires equal member timeline ranges and source durations; otherwise prepare explicit equivalent mappings in an atomic batch. Resample pins `cutcraft.linear-resample` 1.0.0 with empty parameters: pitch follows speed, with linear PCM interpolation and no adaptive antialiasing. Full freeze intervals are silent. Preserve-pitch is unavailable.

`split` and in-range `trim` preserve property and time-map curves. Animated/remapped trim can omit source-range flags; if supplied they must agree with the curve-preserving slice. Static timed trim needs the full new source range. Duration expansion/remapping uses explicit item.put/batch. Loop-phase and legacy nonzero-fade slicing are rejected. Do not drop curves to make a rejected edit pass.

## Transitions

```sh
cutcraft -p PROJECT transition FROM TO --duration 1 --alignment center
cutcraft -p PROJECT transition FROM TO --kind dip-to-color --color '#000000' --duration 1/2 --alignment before
cutcraft -p PROJECT transition FROM TO --duration 1 --progress progress.json
```

Items must be adjacent on the same video track. Windows are before/center/after the cut, must stay within the sequence and pair's outer boundaries, and cannot overlap other transition windows on the track. Media needs real source handles; moving the progress curve does not create handles. Remap handles extend endpoint position using endpoint speed. Built-in color/text generators can extend as constant sources.

Optional progress is a number animation from offset 0/value 0 to offset duration/value 1. Intermediate progress may hold or reverse, but remains in [0,1]. Crossfade and dip-to-color 2.0.0 mix in linear-sRGB with premultiplied alpha; dip color is linear-sRGB. Video transitions do not automatically fade audio: use explicit gain animation when intended.

## Color and LUTs

```sh
cutcraft -p PROJECT item effect ITEM --kind color-grade --parameters '{"exposureEv":0.5,"saturation":1.1}'
cutcraft -p PROJECT import look.cube --managed
cutcraft -p PROJECT item effect ITEM --kind lut --parameters '{"assetId":"LUT_ASSET_ID","inputColorSpace":"rec709","outputColorSpace":"rec709","interpolation":"tetrahedral","strength":0.8}'
cutcraft -p PROJECT item animate ITEM strength --effect EFFECT_ID --file strength.json
```

Read the imported LUT asset ID from output. Select input/output color spaces from the LUT's intended encoding, not its `.cube` extension. Supported 1D LUTs use `linear`; 3D LUTs use `trilinear` or `tetrahedral`. Combined shaper/3D files and Log/HDR are unsupported. `builtin.curves` provides static RGB point arrays; use `schema effect` for ranges and shape.

Animating color-grade numeric parameters or LUT strength sets effect version 2.0.0. Color-grade properties are exposureEv, temperature, tint, contrast, pivot and saturation. LUT asset ID, interpolation/color-space selection and curve point arrays remain static. Effect order matters; unavailable effects must fail rather than silently disappear.

## 高斯模糊（公开 Spec 0.1.0）

`item effect ITEM --kind gaussian-blur --parameters '{"sigma":12}'` 添加内置模糊 1.0.0。
`sigma` 范围 0–100，单位为变换前原始输入像素，默认 0；可用 `item animate ITEM sigma --effect EFFECT_ID --file curve.json` 绑定 owner 时钟 number 动画。
预览自动缩放模糊尺度；效果按数组顺序执行，支持媒体、生成器、嵌套和调整层。内部 0.3/0.4/0.5 不支持此效果，不得改版本字段绕过。

## 模糊背景最佳实践

模糊背景通过现有图层、变换与高斯模糊组合完成，适合竖屏素材填充横屏画布、横屏录屏填充竖屏画布等场景。前景保持清晰，背景铺满画布；直接编辑工程即可，无需预先用 FFmpeg 生成一份模糊视频，也不需要专用内置命令。

1. 已有独立背景画面时直接使用；需要以主体画面作背景时，在下层视频轨创建另一个视觉 Item，复用同一 Asset/Stream 或子序列。保留相同的 `timelineRange`、`sourceRange` 和 `playback`，包括倒放、定格和 timeMap，避免两层不同步。新 Item 和复制的 Effect 使用新 ID，不沿用原 `linkGroupId`。仅复制视觉 Item；嵌套序列的画面和声音也使用独立 Item，不复制对应的音频 Item。
2. 将背景轨排在前景轨下面。轨道数组按从底到顶合成，名称不决定层级；通过 `track.reorder` 提交包含全部轨道 ID 的 `trackIds`。新增轨道、Item 和重排等关联编辑可合并为一次 batch。
3. 背景独立设置为居中、等比铺满画布，前景保留自己的构图。Cutcraft 的 scale 相对 **contain** 尺寸，`1` 不一定铺满；未旋转时，使用裁剪及像素宽高比校正后的输入尺寸 `W × H`、画布 `Cw × Ch`，等比倍率为 `max(Cw/W, Ch/H) / min(Cw/W, Ch/H)`。背景可裁去溢出部分；不要把前景的局部放大或位置动画直接照搬到背景。
4. 仅给背景 Item 加高斯模糊，按画面调整 sigma。前景与背景应覆盖同一时间范围；后续裁剪、变速或改顺序时同步修改两层。用单帧和区间预览检查画布边缘、切点、主体清晰度及是否重复出声。

背景已放到正确轨道并完成构图后：

```sh
cutcraft -p PROJECT item effect BACKGROUND_ITEM --kind gaussian-blur --parameters '{"sigma":20}'
cutcraft -p PROJECT preview --at 1/2
cutcraft -p PROJECT preview --start 0 --duration 2
```

这里的 `BACKGROUND_ITEM` 是背景的实际 ID，预览时间需落在工程的有效范围内。`sigma:20` 是可调整的起点，单位仍是变换前输入像素；效果作用在背景 Item 上，避免误将前景一起模糊。公开 Spec 0.1.0 的高斯模糊支持边界见上一节。
