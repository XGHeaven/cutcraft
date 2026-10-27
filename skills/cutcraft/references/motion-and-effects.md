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
