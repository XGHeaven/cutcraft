# 视觉效果操作（CLI 0.1.0）

以下命令实际修改时间线并参与 render/preview。将 PROJECT、ITEM、ASSET_ID 替换为实际目录或 ID；`inspect` 返回素材、轨道、Item 和效果的 ID。无需 Python。

## 导入和颜色空间

```sh
cutcraft -p PROJECT import video.mp4 --color-space rec709
cutcraft -p PROJECT import look.cube --managed
```

`.cube` 自动识别为 LUT 素材，导入时校验格式并固定 SHA-256。`--managed` 将文件复制到项目 assets；不会修改原文件。LUT 不使用 `--append`，而是被片段效果引用。

有可靠色彩标记的视频可以省略 `--color-space`。无标记或未知颜色空间必须显式声明后才能调色，支持 rec709、srgb、linear-srgb；此声明不是 Log/HDR 转换。已有素材可以通过 `command` 的 `asset.put` 更新 stream.video.colorSpace。

## 偏移、缩放、旋转、裁剪

```sh
cutcraft -p PROJECT item transform ITEM --x 0.6 --y 0.45 --scale-x 1.2 --scale-y 1.2
cutcraft -p PROJECT item transform ITEM --rotation 15 --anchor-x 0.5 --anchor-y 0.5
cutcraft -p PROJECT item transform ITEM --crop-left 0.1 --crop-right 0.1
cutcraft -p PROJECT item transform ITEM --scale-x -1
cutcraft -p PROJECT item transform ITEM --reset
```

未传入的属性保留当前值。位置、锚点和裁剪为归一化值；旋转以度为单位；负倍率镜像、零倍率不可见。`--reset` 清空变换，然后应用本次提供的属性。

## 添加有序效果

```sh
cutcraft -p PROJECT item effect ITEM --kind color-grade \
  --parameters '{"exposureEv":0.5,"temperature":0.1,"tint":0,"contrast":1.1,"pivot":0.18,"saturation":0.9}'

cutcraft -p PROJECT item effect ITEM --kind curves \
  --parameters '{"rgb":[{"x":0,"y":0.02},{"x":0.5,"y":0.55},{"x":1,"y":1}]}'

cutcraft -p PROJECT item effect ITEM --kind lut \
  --parameters '{"assetId":"ASSET_ID","strength":0.7,"inputColorSpace":"rec709","outputColorSpace":"rec709","interpolation":"tetrahedral"}'
```

每次 `item effect` 在堆栈末尾添加效果并返回新的项目快照；顺序影响结果。1D LUT 使用 linear，3D LUT 使用 trilinear 或 tetrahedral。曲线采用分段线性插值，可以分别传入 rgb/r/g/b。

```sh
cutcraft -p PROJECT item effect-remove ITEM EFFECT_ID
cutcraft -p PROJECT undo
cutcraft -p PROJECT validate --level media
cutcraft -p PROJECT render --output output/graded.mp4
cutcraft -p PROJECT preview
```

通过 `command` 的 `item.put` 可进一步修改参数、enabled 或效果顺序；这些编辑同样受锁、revision 和 undo 保护。`schema effect` 可以输出内置参数约束（完整 Effect envelope 见 common Schema）。

## 执行与边界

- 效果可作用于媒体、颜色／文字生成器、嵌套序列和调整层。子序列使用保留 alpha 的无损中间文件，调整层处理下方已合成画面。
- 调色／曲线使用 linear-sRGB，LUT 强度也在线性空间混合。不会把编码后的 RGB 直接拿来混合冒充线性混合。
- 坏文件、LUT 维度与插值不匹配、指纹变化、未知颜色空间、未知效果版本都会报错，既有输出不会被失败任务覆盖。
- 实现为 CPU 流式参考后端；内存按帧使用，中间文件占磁盘，不保证实时预览。效果动画、自定义插件、Log/HDR 和转场不属于这次内置视觉功能。

## 完整演示

```sh
cargo build --release
cargo run --release --example visual_demo -- local-examples/visual-demo.cut
```

目标目录必须不存在。演示程序通过 CLI 导入素材、设置变换、叠加调色／曲线／3D LUT，输出 `output/comparison.mp4`：左为原片，中为曝光加曲线，右为暖色 LUT。
