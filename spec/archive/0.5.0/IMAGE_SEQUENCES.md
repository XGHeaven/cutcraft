# 图片序列规范与实现说明

状态：CLI / Spec 0.5.0 已实现本文件的图片序列来源、导入、搜索与采样接口。使用命令和具体边界见 [图片序列指南](../../../docs/image-sequences.md)。旧 0.4 Schema 和已有项目保持不变；尚无原地升级命令。

## 1. 目标与模型

把一组静态图片登记成一个 Asset，按指定帧率解释为一条视频流。同一素材可以放到多个 Item，复用现有截取、变速、倒放、定格、循环、时间映射、动画、转场和调色能力，无需先编码成中间视频。

- Asset 保持 `kind: "media"`，新增 `source.type: "image-sequence"` 表达物理来源；单张图片仍是 `kind: "image"`，不改变原有行为。
- 图片序列固定只有一条 `kind: "video"`、`index: 0` 的流，初次导入分配 `stream_v0`；不隐含音频。其他音频素材可以独立放在时间线上。
- Item 仍为 `type: "media"`，使用 `(assetId, streamId)` 引用，必须有 `sourceRange`。只有单张静态图片流省略 `sourceRange`。
- `SequenceItem` 仍表示嵌套剪辑序列，与图片序列素材是不同概念。
- 编号模板是导入选择器，不是渲染时的动态文件发现规则。项目保存确定的有序帧清单。

首版支持本地、等帧间隔的图片序列。远程文件、目录监控、自动续帧、可变单帧时长、多层 EXR/HDR 和动图展开不在本次范围；单帧时长变化使用 Item 的 timeMap 表达。

## 2. JSON 表示

以下是 `assets[assetId]` 内的完整示意对象。三张图片以 24 fps 播放，总源时长为 `3/24 = 1/8` 秒。示例文件指纹为占位值，聚合指纹按这些占位值计算，不代表仓库内实际存在的素材。

```json
{
  "name": "喜字动画",
  "kind": "media",
  "uri": "assets/asset_01900000-0000-7000-8000-000000000001/",
  "storage": "managed",
  "availability": "online",
  "fingerprint": {
    "algorithm": "sha256",
    "value": "1826d4b461dadf9500916e139273399372a1108d40d68876bd7699df71a97a11"
  },
  "source": {
    "type": "image-sequence",
    "version": "1.0.0",
    "frames": [
      {
        "uri": "frame_0001.png",
        "fingerprint": {
          "algorithm": "sha256",
          "value": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }
      },
      {
        "uri": "frame_0002.png",
        "fingerprint": {
          "algorithm": "sha256",
          "value": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        }
      },
      {
        "uri": "frame_0003.png",
        "fingerprint": {
          "algorithm": "sha256",
          "value": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        }
      }
    ]
  },
  "streams": [
    {
      "id": "stream_v0",
      "kind": "video",
      "index": 0,
      "duration": { "num": 1, "den": 8 },
      "video": {
        "width": 1920,
        "height": 1080,
        "pixelAspect": { "num": 1, "den": 1 },
        "frameRate": { "num": 24, "den": 1 },
        "colorSpace": "srgb",
        "alphaMode": "straight"
      }
    }
  ]
}
```

字段约束：

| 字段 | 规则 |
| --- | --- |
| `source` | 可选；缺省延续现有单文件素材语义。本提案只新增 `image-sequence` 类型，不凭空引入其他来源类型 |
| `source.version` | 固定为 `1.0.0`，未知版本明确拒绝 |
| `source.frames` | 必须非空，最多 1,000,000 个槽位，数组顺序就是源帧顺序；不再按文件名排序 |
| `frames[i].uri` | 相对于 Asset 的目录 URI；允许子目录，不允许绝对 URI、查询/片段或 `..` 越出根目录；文件名按 URI 规则编码 |
| `frames[i].fingerprint` | 必需，所引用文件字节的 SHA-256 |
| Asset `uri` | 对这种来源表示目录，必须以 `/` 结尾；相对 URI 仍相对于项目目录解析 |
| `storage` | 首版只接受 `managed` 或 `external` |
| `streams` | 恰好一条视频流；`video.frameRate` 必需且为正有理数，是唯一帧率来源 |
| `stream.duration` | 必须等于 `frames.length / frameRate`，使用约分后的有理秒；校验器验证一致性 |
| Asset `fingerprint` | 必需，为下面定义的有序文件集合指纹，不是目录文件的哈希 |

不额外保存 `frameCount`、源帧起始时间、逐帧 PTS 或第二份帧率，它们都可推导。`codec` 不填伪造的容器编码；各帧解码格式由文件决定。

重复引用同一 URI 合法，表示多个源帧槽位使用同一张图片，例如导入时补齐缺帧。同一解析后文件的指纹必须一致。导入时不自动把各帧拆成独立 Asset；需要单独使用某张图时可另行导入。

清单直接保存在 `cutcraft.json`，属于工程事实，不能放进 `cache/`。外置清单只用作导入输入，导入后删除输入清单不影响工程。超长序列会增大快照及现有历史重写成本；首版接受这一成本，不同时增加模板、目录扫描和外置清单三套持久化语义。

## 3. 精确时间与画面采样

设帧数为 N，帧率 R 为正有理数：

```text
第 i 帧的源时间覆盖范围 = [i/R, (i+1)/R)，0 <= i < N
素材源范围              = [0, N/R)
输出时间 → 现有 Item 时间映射 → 源时间 s → 帧清单下标
```

首帧源时间永远为 0。文件从 `frame_1001.png` 开始不意味着素材从 `1001/R` 秒开始。序列输出帧率和素材解释帧率可以不同：24 fps 的素材可以放进 30 fps 的工程，采样仍通过时间映射完成。

默认 `frame-hold` 的普通采样下标为 `floor(s*R)`；采用 `exclusive-upper` 时为 `ceil(s*R)-1`。上下界、非网格 sourceRange、转场 handle 和映射方向规则继承 [0.4 时间重映射](../0.4.0/RETIME.md#5-源边界与画面采样)：源坐标必须先通过范围验证，再选取允许源窗口内覆盖该时间的帧；不能用下标钳位掩盖真正的越界。

举例：24 fps 的第 0 帧覆盖 `[0, 1/24)`。正向采样 `s=1/24` 选第 1 帧；反向在该边界采用左极限时选第 0 帧。在素材尾端 `s=N/R`，只有协议允许的排他上界采样才读取第 N−1 帧。始终用有理数计算，不逐帧累加浮点秒。

`frame-blend` 使用两张相邻源图及精确时间距离求权重，颜色和 alpha 混合规则保持现有定义；不会因为来源变成图片而新增另一套插帧规则。光流仍需明确可用的处理器。倒放、非匀速、定格和循环都只改变源坐标，不改帧清单，不保存第二份速度倍率。

## 4. 导入命令

新增显式命令 `import-sequence`。现有 `import photo.png` 继续导入单张图片，不自动猜测同目录编号文件。

```sh
# 按编号模板导入；引号防止 shell 展开
cutcraft -p demo.cut import-sequence /path/to/frames \
  --pattern 'frame_%04d.png' \
  --fps 24 --color-space srgb --managed --append

# 可选 Nuke 风格写法；显式选择，避免不同工具的 # 语义冲突
cutcraft -p demo.cut import-sequence /path/to/frames \
  --pattern 'frame_####.png' --pattern-style hash \
  --fps 24 --managed

# 明确编号范围和缺帧策略
cutcraft -p demo.cut import-sequence /path/to/frames \
  --pattern 'frame_%04d.png' --start-number 1001 --end-number 1120 \
  --fps 30000/1001 --missing hold --managed

# 不规则命名：顺序由 JSON 数组明确给出
cutcraft -p demo.cut import-sequence /path/to/frames \
  --files ordered-frames.json --fps 24 --managed
```

`--pattern` 与 `--files` 必须且只能选择一个。`--fps` 必须提供，不能从文件名、EXIF 时间或工程输出帧率猜测。`--managed`、`--append`、`--color-space` 延续已有命令含义。

### 4.1 编号模板

#### 主流约定与采用范围

调研日期：2026-09-29。以下描述各工具公开约定；Cutcraft 的具体范围以本节后面的规则为准。

| 实现 | 编号写法与相关行为 | Cutcraft 的选择 |
| --- | --- | --- |
| FFmpeg image2 | `%d`、`%0Nd`，`%%` 表示字面百分号；sequence 与 glob 是不同模式。sequence 起始编号默认 0，默认在其后 5 个编号内查找首帧，之后要求连续编号 | 默认采用其文档化的 printf 模板语法；完整目录发现和显式缺帧策略由 Cutcraft 自己执行，不照搬起始查找窗口。[官方文档](https://ffmpeg.org/ffmpeg-formats.html#image2-1) |
| Nuke | `####` 表示四位补零，`###` 表示三位，数量表达宽度 | 作为可选 hash 风格的宽度约定。[官方文档](https://learn.foundry.com/nuke/content/comp_environment/rendering/file_name_conventions.html) |
| OpenImageIO oiiotool | 同时提供 printf、范围表达式以及 `#` / `@`；一个 `#` 表示四位，一个 `@` 表示一位；缺帧默认使执行失败，可显式跳过失败帧 | 不自动猜测 # 方言，不引入内嵌范围、@、视图变量等完整表达式语言。[官方文档](https://openimageio.readthedocs.io/en/latest/oiiotool.html#frame-sequences) |

关键修正：`%04d` 的 4 是**最小宽度**，不是恰好四位。FFmpeg 的实现使用整数零填充格式化，数值超过宽度时不会截断；例如 9999 后面是 10000。[FFmpeg 源码](https://github.com/FFmpeg/FFmpeg/blob/master/libavformat/utils.c)

#### 默认 printf 风格

`--pattern-style printf` 为默认值。只解析以下文档化子集，不把整个字符串交给 printf 或 shell：

| 写法 | 含义 | 例子 |
| --- | --- | --- |
| `%d` | 无补零的十进制编号 | `frame_1.png`、`frame_10.png` |
| `%0Nd` | 左侧补零至至少 N 位 | `%04d` → `0001`、`0010`、`9999`、`10000` |
| `%%` | 字面 `%` | `rate_50%%_%04d.png` → `rate_50%_0001.png` |

- 模板仅作用于 DIRECTORY 下的文件名，恰好包含一个编号槽。前缀、后缀逐字符匹配；`#`、`@`、`*`、`?`、括号等在这种风格中都是普通字符。不递归，不把通配符当 glob，不做路径变量/环境变量替换。
- 未知或不完整的 `%` 写法、多个编号槽、没有编号槽均报 `INVALID_SEQUENCE_PATTERN`。不接受 `%s`、`%f`、`%4d`、日期格式、正则或任意格式化表达式。N 的文本形式为无前导零的 1–19；例如 `%04d` 有效，`%004d` 不在子集中。
- 首版编号仍限制为非负 signed-64 整数；负编号输入可用显式清单。这是 Cutcraft 的范围限制，不宣称覆盖 FFmpeg 格式化器的全部能力。
- 匹配采用“提取数字 → 解析编号 → 按模板重新格式化 → 与原文件名精确比较”。因此 `%d` 不接受 `0001`，`%04d` 接受 `0001` 和 `10000`，但不接受 `1` 或 `00001`。不会把多个不同补零方式模糊合并成同一序列。
- 数字槽为 ASCII `0–9`，前后缀支持中文等 Unicode 文本；匹配不做大小写折叠或 Unicode 归一化。同目录多个版本通过模板的固定前缀区分，例如 `shot_v002.%04d.png` 的 `002` 只是文件名的一部分。

#### 可选 hash 风格

`--pattern-style hash` 显式启用 Nuke 风格的“每个 # 一位”，将唯一一个连续 # 块编译成上述内部编号槽。`####` 等价于 `%04d`，`#` 等价于 `%01d`；补零最小宽度和 round-trip 匹配规则由 Cutcraft 按上面同一套语义定义。连续 # 数量限制为 1–19。

hash 模式的 `%`、`@` 是普通字符，不能再包含第二个 # 块。需要字面 # 文件名时，使用默认 printf 模式或显式文件清单。`--pattern-style` 只能与 `--pattern` 一起使用，不能与 `--files` 混用。不提供 auto 模式；例如 OIIO 的 `shot.#.png` 不能被默认为四位，应改用 `shot.%04d.png`。

两种风格内部都只保留 `{prefix, suffix, padding}` 的导入选择器；它不进入渲染用的帧清单。`padding=0` 表示 `%d`，正数表示最小补零宽度。

#### 编号范围、发现和固定清单

- 扫描 DIRECTORY 的文件名，按上述模板识别候选，以数值顺序排列。不能用字典序、文件修改时间或目录遍历顺序代替编号顺序。
- 范围闭区间 `[start-number, end-number]`。显式端点约束候选；缺省端点从满足显式端点约束的候选中推导最小/最大编号。所有端点确定后，区间内缺口按 `--missing` 处理；未显式指定的首尾范围外不存在可推断的缺帧。
- 当两个端点都显式给出时，保留整个区间，即使边界文件缺失也不得把端点移到第一/最后现存文件；交给缺帧策略判断。没有可用图片、范围倒置、数值溢出或清单过大无法分配时明确报错。
- Cutcraft 故意不采用 FFmpeg 默认的 0–4 首帧搜索窗口：目录只有 1001–1100 时可以自动发现，也可以通过显式端点固定范围。
- 仅范围内成员参与导入，编号不要求从 0 或 1 开始。首次导入生成清单后，目录中新建、更名或删除其他文件不会改变成员集合。
- glob 没有明确的编号槽和缺帧语义，首版不将它混入 `--pattern`。不规则文件名使用有序 `--files` 清单；若未来增加 glob，应作为独立选择模式定义排序规则。

模板匹配规则：

| 模式 / 模板 | 文件名 | 预期 |
| --- | --- | --- |
| printf / `frame_%d.png` | `frame_1.png` / `frame_01.png` | 接受编号 1 / 不匹配 |
| printf / `frame_%04d.png` | `frame_0001.png` / `frame_1.png` | 接受编号 1 / 不匹配 |
| printf / `frame_%04d.png` | `frame_10000.png` / `frame_00001.png` | 接受编号 10000 / 不匹配 |
| printf / `rate_50%%_%04d.png` | `rate_50%_0001.png` | 接受编号 1 |
| hash / `frame_####.png` | `frame_0001.png` | 接受编号 1 |
| hash / `frame_#.png` | `frame_0001.png` | 不匹配；不是 OIIO 的单 # 四位语义 |
| printf / `frame_####.png` | 任意 | 模板错误：没有编号槽 |
| printf / `v%02d_frame_%04d.png` | 任意 | 模板错误：两个编号槽 |

### 4.2 显式文件顺序

`ordered-frames.json` 的内容例如：

```json
["开场.png", "细节.png", "细节.png", "结束.png"]
```

输入清单路径按 CLI 当前工作目录解析；数组成员是相对于命令中 DIRECTORY 的文件路径，不是 URI 编码字符串，导入时转换为帧 URI。按数组顺序导入，不重新排序，允许重复项；空数组或任一文件缺失都报错。编号范围和 `--missing` 只适用于模板模式，不能与 `--files` 混用。

### 4.3 缺帧策略

假设选定范围为 1–4，但只有 1、2、4 号文件：

| `--missing` | 行为 | 固定写入的帧清单 |
| --- | --- | --- |
| `error`（默认） | 拒绝导入，返回缺失编号 | 无提交 |
| `hold` | 用选定范围内前一张可用图填充缺失槽位，保留原时长 | `[1, 2, 2, 4]` |
| `skip` | 删除缺失槽位，序列缩短；不会保留时间空洞 | `[1, 2, 4]` |

`hold` 遇到选定范围的第一帧缺失仍报错，不读取范围外前一张图，也不使用未来帧。重复项只是重复 URI 和指纹，受管导入不重复复制文件。

这些策略只在**导入时**应用。导入之后清单中的文件丢失、损坏或指纹变化，校验/渲染必须报错，不能再按 `hold` 或 `skip` 静默补救。之后出现原先缺失的文件，也不会替换清单中的重复项。

### 4.4 导入输出与初始 Item

输出提供 `assetId`、`streamId`、`sourceType`、帧率、总帧数、不同文件数、精确时长、实际采用的缺帧策略和缺帧编号压缩区间。`--append` 额外返回新建的 sequence/track/item ID。大清单不在默认响应里完整重复，可另行分页查询。

`--append` 初始使用完整源范围，timelineRange.duration 与 sourceRange.duration 都为 `N/R`，没有默认 5 秒图片时长，也没有音频片段。新增命令的登记和可选 append 编译成一次原子 batch；不顺带改变旧 `import` 命令的事务行为。

## 5. 文件、颜色与指纹

### 5.1 图片要求

每个不同文件都必须可解码为单张静态图。首版支持单张图片路径已有的解码格式，但应拒绝多帧 GIF/APNG/动画 WebP/多页 TIFF 等多画面输入，不静默取第一帧。不可仅按后缀判断。

所有帧必须具有相同解码尺寸、像素宽高比，并可按同一已知颜色空间解释。允许兼容的静态 PNG/JPEG 等格式混用；不透明图在存在透明帧时按 alpha=1 处理，流统一声明 straight alpha。首版不接受需要未实现 HDR、预乘 alpha 或复杂通道语义的图片。

导入检查整组文件，不能只探测第一张。不自动调整尺寸、应用 EXIF 旋转或把未知颜色空间标成 sRGB；现有解码器采用不自动旋转的像素方向，尺寸比较也以此为准。`--color-space` 是整组文件的解释声明，不是颜色转换；已知帧颜色声明互相冲突时拒绝，需预先规范化。未知但有依据的统一源空间才由用户显式指定。

### 5.2 外部与受管存储

- `external`：Asset URI 指向原目录，帧 URI 是该目录下的相对路径。不复制或修改源图。
- `managed`：复制到项目的 `assets/<assetId>/`，保持所选文件的目录结构与名称。同一源文件在清单中重复出现时只复制一次。文件不会放到 `cache/`。
- 移动整个工程时，受管序列不依赖原路径。重新链接外部序列根目录只需更新 Asset URI 并验证每帧指纹，素材及流 ID 保持不变。

受管文件先复制到本次导入专用的临时目录并校验指纹，完成后再发布最终目录和提交项目。文件发布与现有持久化日志协调：失败前没有日志引用的临时数据可清理；出现待恢复事务后不得删除其可能引用的素材。进程在文件发布后、项目提交前退出，最多留下未引用的受管目录，不允许工程引用不完整的目录。不将清理整个 assets/ 纳入导入流程。

### 5.3 聚合指纹

保持已有 `fingerprint.algorithm: "sha256"` 外壳；语义由 `source.type/version` 决定。图片序列的值定义为：

```text
SHA256(
  UTF8("cutcraft.image-sequence.v1") || 0x00 ||
  uint64_big_endian(N) ||
  raw_sha256(frame[0]) || ... || raw_sha256(frame[N-1])
)
```

`raw_sha256` 是每帧记录的 32 字节哈希值，不是十六进制字符串。重复帧也参与，帧顺序改变则集合指纹改变；仅移动或重命名文件不改变内容指纹。帧率和颜色声明是解释参数，不属于内容指纹，但必须进入渲染缓存键和项目快照哈希。

身份与指纹分离：相同内容可以有不同 Asset ID；改变内容也不能偷偷生成新的 ID。改变帧率、成员或顺序要通过显式编辑提交完整 Asset，并同时验证所有引用它的 sourceRange，不能仅改 duration 或让依赖片段自动漂移。

## 6. 搜索、定位与查看帧

复用当前搜索接口，并作以下扩展：

| 接口 | 行为 |
| --- | --- |
| `asset list/search --source-type image-sequence` | 按来源筛选；`--kind media` 仍包含序列 |
| `asset search --path 'frame_0042.png'` | 同时搜索序列根路径和成员的相对/解析后路径，返回所属 Asset，并给出首个匹配的帧索引及匹配数；不为每帧建立 Asset |
| `asset info ASSET_ID` | 返回完整素材；增加可选 `--summary`，省略逐帧清单，返回帧数、不同文件数、帧率、时长、根路径 |
| `asset frames ASSET_ID --offset 0 --limit 100` | 新增零基分页帧清单，包含 index、URI、解析路径、精确源时间范围、指纹；limit 默认 100，上限 1000 |
| `asset usages ASSET_ID` | 继续返回引用该素材的 Item，包括 sourceRange 和所在序列的 timelineRange |
| `item search` | 素材路径匹配扩展到图片序列成员路径；结果仍是一行一个 Item |
| `sample --at ...` | 对图片序列增加计算出的源帧索引、URI 和混合权重，供 Agent 定位实际所用帧；不解码图片 |

`asset list/search` 默认只返回序列摘要，不能输出数万帧清单。`asset frames` 是查看记录，不自动生成缩略图或截图。帧索引是清单下标，不等同于文件名编号。

通过成员路径找到 Asset，再查询 usages，返回的是引用整个序列的片段，不表示每个片段都采到了这张图。若要确认某一输出时刻实际采到哪些图，使用增强后的 sample；暂不新增“某张源图在任意非匀速/循环曲线下对应哪些输出区间”的反向求解器。

## 7. 渲染与缓存

媒体解析按 `source.type` 分支：缺省走当前单文件解码器，image-sequence 走清单帧解码器。上层仍通过素材源时间请求帧，不需要为图片序列复制一套时间映射或效果管线。

图片序列按需解码清单中的文件，采用有界缓存。frame-hold 通常只需一帧，frame-blend 需要相邻两帧；反向和随机映射可以直接定位图片，无需先把整个序列解码成大 raw 文件。启动渲染时验证清单和所需文件；正式媒体完整校验覆盖全部不同文件。可复用校验索引，但不能仅凭目录修改时间绕过文件指纹检查。

当前解码帧使用单次渲染的 64 MiB LRU 内存缓存，键包含内容指纹、颜色/alpha 解释、解码配置与尺寸；同次渲染使用固定解码器，超过预算的单帧不缓存。没有跨运行的解码缓存；以后若持久化必须加入解码器版本。预览缓存路径包含 CLI 版本、项目内容哈希和渲染档位，清单/帧率都参与项目哈希。预览沿用小画布/帧率上限，正式导出保留项目设置。预览 MP4 放在 `cache/`，原图和清单不属于缓存。

## 8. 版本、实现范围和验收

使用 Spec 0.5.0，因为 0.4.0 Asset Schema 不允许 `source`，旧后端把 Asset URI 一律当单文件。保持 `source` 可选，新版读旧素材时不自动增加字段。

CLI 0.5.0 新建项目采用本版本，并嵌入独立 0.5 Schema；0.3/0.4 项目继续按旧协议处理。向旧项目新增图片序列返回 `PROJECT_UPGRADE_REQUIRED`，不在 import 中偷偷改版本。当前需要新建 0.5 项目；原地升级仍属于单独的版本迁移工作，不由 `asset.put` 绕过。

实现覆盖：

1. Schema/示例/语义校验：新增来源分支，帧率和时长约束、指纹算法、路径规则。
2. 导入、文件发布及原子 batch，含编号/显式清单和缺帧策略。
3. 清单解码适配，接入已有时间映射、效果、sample、preview/render。
4. 媒体校验、搜索定位、分页帧清单、文档/技能更新。

验收应至少覆盖：

- 非 0 起始编号、中文/空格路径、重复编号、自然数字顺序、显式清单顺序及重复项。
- error/hold/skip 的时长差异，首帧缺失，导入后新增/丢失/变化文件不会重写成员集合。
- 24、30000/1001 fps 的精确时长与帧边界，sourceRange 裁剪，末端倒放、定格、非匀速、循环、frame-blend。
- 颜色/尺寸/alpha 不一致，多画面输入被拒绝，透明 PNG 正确参与合成。
- 同一素材多次使用、帧路径搜索、嵌套序列定位和 disabled 引用计数。
- 受管工程搬家、根目录重新链接、指纹变化、复制或提交中断恢复；不修改原图。
- 预览小尺寸、正式导出正常尺寸；大量帧时缓存有界，不把整组 raw 帧装进内存。

`tests/image_sequence.rs` 覆盖实际导入、查询、采样、渲染、失败与兼容性场景；通用编辑/动画/持久化场景同时运行原有回归测试。当前完整媒体校验仍逐文件检查，不宣称已优化大规模序列的校验耗时。
