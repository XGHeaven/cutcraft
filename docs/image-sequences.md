# 图片序列

CLI 0.1.0 原生支持将一组静态图片导入为一个素材、一条视频流，无需先编码成视频。新建项目使用 Spec 0.1.0；已有 0.3/0.4 项目仍可编辑和导出，但图片序列导入会返回 `PROJECT_UPGRADE_REQUIRED`。内部 0.5.0 工程仍支持图片序列。目前没有原地升级命令，0.3/0.4 需要新建 0.1.0 项目；不要直接改旧项目的版本字段或历史。

## 导入

```sh
cutcraft init animation.cut --name 'Image animation'
cutcraft -p animation.cut import-sequence /path/to/frames \
  --pattern 'frame_%04d.png' --fps 24 \
  --color-space srgb --managed --append
```

`--fps` 必填，支持 `24`、`30000/1001` 等精确有理数。编号从 1001 开始也可以，首张图的素材时间仍为 0。`--color-space` 描述原图的颜色空间；缺少可靠颜色标记时必须显式声明，不能把它当成转换 Log/HDR 的选项。

`--managed` 将文件复制到 `assets/<assetId>/`，不改原图；不传则引用原目录。`--append` 把全长视频片段加入主序列末尾；登记和 append 为同一事务，没有默认五秒图片时长。返回 `assetId`、`streamId`、精确时长、帧数、不同文件数、缺失编号区间和 `append` 中的 sequence/track/item ID。导入不支持 `--dry-run`。

不自动把普通 `import photo.png` 扩展成序列，也不会在渲染时重新扫描目录。

## 模板与清单

默认 `--pattern-style printf`：

| 模板 | 匹配示例 |
| --- | --- |
| `frame_%d.png` | `frame_1.png`、`frame_10.png`；不匹配 `frame_01.png` |
| `frame_%04d.png` | `frame_0001.png`、`frame_9999.png`、`frame_10000.png`；不匹配 `frame_1.png`、`frame_00001.png` |
| `rate_50%%_%04d.png` | `rate_50%_0001.png` |

宽度是最小补零宽度，支持 1–19。只允许一个编号槽；`v002_frame_%04d.png` 中的 `002` 是字面内容。模板只匹配目录中的文件名，不递归、不解释 glob/正则，数字按数值排序。

Nuke 风格需要显式选择：

```sh
cutcraft -p animation.cut import-sequence /path/to/frames \
  --pattern 'frame_####.png' --pattern-style hash --fps 24 --color-space srgb
```

hash 风格每个 `#` 表示一位，`####` 等价于 `%04d`；不采用 OpenImageIO 的单个 `#` 四位规则。

不规则命名或自定义顺序使用 JSON 数组：

```json
["开场.png", "细节.png", "细节.png", "结束.png"]
```

```sh
cutcraft -p animation.cut import-sequence /path/to/frames \
  --files ordered-frames.json --fps 24 --color-space srgb --managed
```

清单文件路径相对于当前工作目录；数组成员是相对于图片目录的原始文件路径，不能越出该目录。允许子目录和重复文件，重复项不重复复制。文件名中的 `%` 等字符按字面读取。清单会写入项目，之后删除导入用的 JSON 文件不影响项目。

## 范围与缺帧

```sh
cutcraft -p animation.cut import-sequence /path/to/frames \
  --pattern 'frame_%04d.png' --start-number 1001 --end-number 1120 \
  --missing hold --fps 24 --color-space srgb
```

起止编号都包含在范围内，省略时从匹配文件推导。缺帧默认 `error`；`hold` 重复选定范围内前一张图并保留时长，首帧缺失仍报错；`skip` 压缩缺口，缩短源时长。这些参数仅可用于模板模式。

策略只执行一次：导入后新增文件不会改变成员，原文件丢失或内容变化会导致校验/渲染失败，不会静默补帧。

## 查找与使用

```sh
cutcraft -p animation.cut asset list --source-type image-sequence
cutcraft -p animation.cut asset search --path frame_0042.png
cutcraft -p animation.cut asset info ASSET_ID --summary
cutcraft -p animation.cut asset frames ASSET_ID --offset 0 --limit 100
cutcraft -p animation.cut asset usages ASSET_ID
cutcraft -p animation.cut item search frame_0042.png
cutcraft -p animation.cut sample --at 1/2
```

帧清单零基分页，limit 默认 100，允许 1–1000。返回 index、文件 URI、解析路径、源时间范围和指纹；列表/摘要不展开完整帧清单。搜索包含成员路径，并返回首个命中的下标和命中帧槽位数量。

`asset usages` 定位引用整个素材的片段；某个片段可能只使用其中一段，不保证它实际采到搜索的那张图。`sample` 的 `imageSequenceFrames` 返回当前采样的帧索引、URI 和有理数权重，含帧混合及嵌套序列中的结果。

普通 `item add` 使用该素材的 `stream_v0` 和 `sourceRange`。截取、倒放、定格、循环、时间映射、动画、转场、调色/LUT 沿用视频片段规则；`preview` 和 `render` 直接读取源图。

## 边界与性能

- 单组最多 1,000,000 个帧槽位；模板选定编号范围也最多 1,000,000。
- 要求各图解码尺寸、像素宽高比和颜色解释一致。统一解码为 straight RGBA，不透明图 alpha=1；保留透明图片的 alpha。
- 本地静态 PNG/JPEG/WebP/BMP/TIFF，拒绝动画 PNG/WebP、多页 TIFF 等多画面容器；不自动应用 EXIF 旋转、缩放尺寸或转换未知 HDR/Log。
- 导入和媒体校验检查全部不同文件。渲染仍先运行完整媒体校验，再按采样需要解码；大序列的校验有 IO/进程启动成本。
- 解码帧使用单次渲染的 64 MiB LRU 缓存；单张图大于预算则不缓存。无需生成整段 raw 序列。预览 MP4 在 `cache/`，原图和清单不属于缓存。
- 协议支持有序逐帧指纹及 URI；快照和历史体积会随清单增长。没有目录监控、自动去重、跨运行解码缓存或原地协议升级。

完整存储和采样规则见 [图片序列规范](../spec/IMAGE_SEQUENCES.md)。
