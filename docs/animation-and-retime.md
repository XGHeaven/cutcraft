# 动画、转场和时间映射（CLI 0.1.0）

新建项目使用 `cutcraft.json`、`specVersion: "0.1.0"`，完整规范和 Schema 位于 [spec/](../spec/README.md)。内部 0.3/0.4/0.5 工程按原版本读取、编辑和渲染，不隐式迁移；0.3 不支持本页的动画与时间重映射字段。

## 动画

动画文件保存完整 `$animation` 对象，`--file -` 从 stdin 读取，无需 Python：

```sh
cutcraft -p film.cut item animate ITEM video.transform.rotationDegrees --file - <<'JSON'
{"$animation":{"version":"1.0.0","valueType":"number","timeReference":"owner","keyframes":[{"offset":{"num":0,"den":1},"value":0},{"offset":{"num":2,"den":1},"value":360}]}}
JSON
```

支持 `video.opacity`，`video.transform.position/anchor/scale` 整向量或 `.x/.y`，`rotationDegrees`，`crop.left/right/top/bottom`，以及 `audio.gainDb/pan`。number/vector2 支持 linear、hold、cubic-bezier；最后一个关键帧不能带 easing。hold 在下一关键帧时跳变。片段内使用 owner 局部时间；父片段的动画不随素材倒放反向，嵌套子序列的动画则随子时间映射执行。

```sh
cutcraft -p film.cut item effect ITEM --kind color-grade --parameters '{"exposureEv":0}'
cutcraft -p film.cut item animate ITEM exposureEv --effect EFFECT_ID --file exposure.json
cutcraft -p film.cut item animate ITEM strength --effect LUT_EFFECT_ID --file strength.json
```

为效果设置动画时，CLI 将对应 color-grade/LUT 效果版本设为 2.0.0；gaussian-blur 的 sigma 从效果 1.0.0 起支持动画，保持该版本。曲线点数组、LUT 资源、插件不认识的参数不能动画化。恢复静态值可使用 `item transform` 或完整的 `item.put`；替换整向量动画之后才能单独修改它的分量。

## 匀速和非匀速

```sh
cutcraft -p film.cut item speed ITEM --rate 2 --animations stretch --ripple remove-gap
cutcraft -p film.cut item speed ITEM --rate -0.5 --animations keep-local
cutcraft -p film.cut item time-map ITEM --file mapping.json --animations keep-local
```

speed 固定源范围，根据有理数倍率计算时间线时长；负号表示倒放。项目不保存另一份独立 speed 字段。替换已有 timeMap 需要 `--replace-remap`。零倍率不能推导时长，用显式 timeMap 定格。

`mapping.json` 示例：

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

前两秒平滑加速，接着定格一秒，最后倒放两秒。value 是相对于 sourceRange.start 的源偏移。最后一个 offset 决定新时间线时长，源窗口保持不变，至少需要容纳这些位置和 Hermite 控制值。timeMap 只接受有理数 linear/cubic-hermite，禁止浮点源时间和 hold 跳剪。

每个输出时刻独立求源坐标，不通过逐帧累加倍率计算。内侧边界按映射方向选择，范围上界使用前一帧；不加浮点 epsilon。frame-hold 按真实 PTS 选择覆盖帧，frame-blend 按源 PTS 在**线性光、预乘 alpha**空间混合。可通过完整 item.put 设置 `playback.videoSampling: {"method":"frame-blend"}`。解码器支持有明确时间戳、末帧时长的 CFR/VFR 视频，拒绝缺失/重复/无序时间戳。

默认 `--animations keep-local` 保留属性关键帧局部时间，缩短后越界则整个操作失败；`stretch` 按新旧时长比缩放属性关键帧 offset。`--ripple none` 不移动后续片段，`insert/remove-gap` 将受影响轨道的后续片段按时长变化量移动，链接成员一起移动一次。标记不跟随移动。

链接 A/V 会在一次事务内一起变速。便捷命令要求成员时间线范围和源时长相同，不满足时应提交包含各自明确映射的 batch。音频非 +1 速必须指定政策：

```sh
cutcraft -p film.cut item speed VIDEO_ITEM --rate 2 --audio-policy mute
cutcraft -p film.cut item time-map VIDEO_ITEM --file mapping.json --audio-policy resample
```

resample 保存固定处理器 `cutcraft.linear-resample`、版本 `1.0.0`、空 parameters。该参考处理器对 PCM 做线性插值，音高随速度变化，支持倒放；它没有自适应抗混叠滤波。完整定格区间输出静音。`preserve-pitch` 和 `optical-flow` 的协议接口可存储，但没有安装对应处理器，渲染明确返回 `PROCESSOR_UNAVAILABLE`，不降级。

## 转场

```sh
cutcraft -p film.cut transition FROM_ITEM TO_ITEM --duration 1 --alignment center
cutcraft -p film.cut transition FROM_ITEM TO_ITEM --kind dip-to-color --color '#000000' --duration 1/2 --alignment before
cutcraft -p film.cut transition FROM_ITEM TO_ITEM --duration 1 --progress progress.json
```

两个片段须在同一视频轨相邻。before/center/after 决定窗口相对剪辑点的位置；progress.json 是从 offset=0/value=0 到 offset=duration/value=1 的 number 动画，可停顿或回退。两种内置转场均为 2.0.0，按线性光、预乘 alpha 混合；dip 的颜色参数也是线性 sRGB。窗口不重叠、不得超出序列/片段外侧范围，素材必须有真实源余量。timeMap 的 handle 用端点位置和单侧速度线性外推。转场不自动给音频加淡化。

## 编辑和验证

`item split` / `item trim` 对属性动画保留局部函数，贝塞尔区间重新参数化，Hermite 用精确位置和导数重建。曲线片段的 trim 仅支持裁剪原范围，可省略 source-range 参数；若传入则必须等于保真裁剪推导的范围。扩展或重写映射请使用原子 item.put/batch。loop 任意相位切分、旧式非零 fade 字段的切分仍明确拒绝。

```sh
cutcraft -p film.cut sample --at 1
cutcraft -p film.cut validate --level media
cutcraft schema animation
cutcraft -p film.cut render --output output/final.mp4
cutcraft -p film.cut undo
```

sample 返回求值后的注册属性、精确源时间/边界方向，以及活动转场两侧的角色和进度。跨属性 crop 限制在实际渲染采样点检查，非法采样导致导出失败，旧输出保持原样。项目编辑沿用 OS 锁、revision、原子 batch 和 history/inverse。

当前渲染是离线 CPU 参考实现。媒体先解码为临时磁盘索引，逐输出帧和音频采样求值；内存不保留整段视频，但未压缩中间文件可能很大（RGBA16 为每像素 8 字节）。目前不适合实时预览或长时间 4K 导出。图片/视频必须有明确 rec709/srgb/linear-srgb 声明；无可信元数据时导入需 `--color-space`。输出为 H.264/AAC MP4、Rec.709，支持方形像素和单声道/立体声。

## 可复现演示

```sh
cargo build --release
cargo run --release --example animation_demo -- local-examples/animation-demo.cut
```

演示通过 CLI 创建项目和全部编辑，自动生成测试素材，导出位移/旋转、平滑变速、定格、倒放、交叉淡化与 dip-to-white 的八秒视频。用户原素材不参与这个演示。
