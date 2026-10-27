# Cutcraft 0.1.0：变换、调色、曲线、LUT 与高斯模糊

本文是 SPEC.md 的规范性组成部分。Schema 验证结构；跨字段约束由 Semantic 校验执行；文件内容、色彩转换与后端能力由 Media 校验执行。保存效果不表示所有后端都能执行它。未实现的启用效果 MUST 报错，不得静默忽略。

## 1. Item 变换

已有 `item.video.transform`，无需新增素材层面的属性。同一素材的各个 Item 可有不同变换，Asset 原文件不变。

- `position`：锚点的目标位置，归一化到序列画布，默认 `{x:0.5,y:0.5}`；允许超出画布。相对偏移通过修改 position 表达，例如向右移动画布宽的 10% 就将 x 加 0.1。
- `anchor`：裁剪后源矩形内的归一化锚点，默认 `{x:0.5,y:0.5}`。这是源局部坐标，不是目标画布坐标。
- `scale`：相对于保持宽高比 contain 适配尺寸的倍率，默认 `{x:1,y:1}`。允许非等比、负值镜像；零值使该轴退化、无可见面积。
- `rotationDegrees`：以锚点为中心顺时针旋转，默认 0。
- `crop`：先从源的上下左右裁剪归一化比例，各边默认 0，对边之和必须小于 1。

几何顺序固定为：校正源像素宽高比 → crop → contain 适配画布 → scale → 绕 anchor 旋转 → 将 anchor 平移到 position → 画布边界裁切。未覆盖区域保持透明。

例：在居中的基础上右移 10%、向上 5%、放大至 1.2 倍：

```json
{"video":{"transform":{"position":{"x":0.6,"y":0.45},"anchor":{"x":0.5,"y":0.5},"scale":{"x":1.2,"y":1.2},"rotationDegrees":0}}}
```

## 2. 效果堆栈

`item.effects` 是有序数组，每个 Effect 有稳定 `id`、`effectType`、固定 `version`、`enabled`（默认 true）和 `parameters`。参数属于 Item，而不是共享 Asset。

本文定义的效果只允许在视频轨 Item 上使用，包括 media、sequence、generator 和 adjustment。处理顺序：源颜色解码/转换 → 按数组次序执行效果 → 几何变换 → Item opacity → 图层混合。adjustment 以自身轨道以下已经合成的画面为输入，只在其 timelineRange 内生效；按轨道自下而上、轨道内 Item 的 canonical 顺序求值。它支持 video.opacity（处理结果与原合成画面的混合强度）及 video.transform；未覆盖区域保留原合成画面。

颜色效果使用非预乘 RGB，保持 alpha 不变，合成前再预乘。`builtin.color-grade` 和 `builtin.curves` 在 **linear-sRGB、D65、sRGB/Rec.709 原色**空间运行；后端负责显式转换并在效果间保持颜色空间一致。无法确定输入传递函数、原色或不支持转换时必须报错，不能仅凭文件名猜测。下面 color-grade、curves、lut 的 `1.0.0` 参数仅支持静态值；gaussian-blur 1.0.0 从首次定义即支持 sigma 动画。`builtin.color-grade` / `builtin.lut` 另支持 `2.0.0`，在注册参数中允许内联 `$animation`，数值公式不变；`builtin.curves` 仍为 `1.0.0`。完整绑定和限制见 [ANIMATION.md](ANIMATION.md)。本版 Effect 不允许旧 automation 字段。

### 2.1 builtin.color-grade / 1.0.0

| 参数 | 范围 | 默认 | 定义 |
|---|---|---|---|
| exposureEv | -20…20 | 0 | 各通道乘以 `2^exposureEv` |
| temperature | -1…1 | 0 | 红通道乘 `2^temperature`，蓝通道乘 `2^-temperature` |
| tint | -1…1 | 0 | 绿通道乘 `2^tint` |
| contrast | 0…4 | 1 | 线性对比度斜率 |
| pivot | 0.000001…1 | 0.18 | 对比度中心 |
| saturation | 0…4 | 1 | 围绕亮度的饱和度系数 |

固定顺序：曝光 → temperature/tint 通道增益 → 对比度 `c=(c-pivot)*contrast+pivot` → 饱和度 `c=Y+(c-Y)*saturation`，其中 `Y=0.2126R+0.7152G+0.0722B`。只在效果输出处将 RGB 截断至 `[0,1]`。temperature/tint 是确定性的通道增益风格调节，不表示开尔文白平衡或摄像机 RAW 白平衡。

### 2.2 builtin.curves / 1.0.0

参数为可选 `rgb`、`r`、`g`、`b` 曲线，每条是至少两个 `{x,y}` 点组成的数组。x/y 都在 `[0,1]`，x 严格递增，首点 x 必须为 0，末点 x 必须为 1。缺省曲线为 identity。

先将输入截断到 `[0,1]`，对每个通道应用公共 rgb 曲线，再应用对应的 r/g/b 曲线。相邻点采用分段线性插值，不做隐含平滑或样条插值。曲线可降低亮部或抬高黑位，因此 y 端点无需为 0/1。

### 2.3 builtin.lut / 1.0.0

```json
{"id":"effect_01a05303-e41c-71db-94d6-b46f69526ef7","effectType":"builtin.lut","version":"1.0.0","enabled":true,"parameters":{"assetId":"asset_01a05303-e41c-71db-94d6-b46f69526ef7","strength":1,"inputColorSpace":"rec709","outputColorSpace":"rec709","interpolation":"tetrahedral"}}
```

- assetId MUST 引用 `assets` 中 `kind: "lut"` 的 Asset，包括禁用效果也必须保持引用有效。LUT 不可直接作为时间线 media Item 使用。
- LUT Asset 使用已有 uri/storage/availability/fingerprint；streams 必须为 `[]`；正式渲染 MUST 固定并校验 SHA-256。
- strength 在 `[0,1]`，默认 1。
- inputColorSpace 和 outputColorSpace 必填：`linear-srgb`、`srgb` 或 `rec709`。这是 LUT 实際期望/产生的信号编码，不是文件扩展名推断出的属性。此版本不表示 Log 摄像机曲线；此类 LUT 需要未来扩展色彩空间。
- interpolation 必填：1D 必须为 `linear`，3D 必须为 `trilinear` 或 `tetrahedral`；文件内容和参数不匹配是 Media 错误。
- 将输入变换到 inputColorSpace，按 DOMAIN 归一化并截断后查表，再将结果从 outputColorSpace 转回工作空间。最终 `out=(1-strength)*original + strength*look`，两项必须在同一 linear-sRGB 空间；保留 alpha。

### 2.4 builtin.gaussian-blur / 1.0.0

```json
{"id":"effect_01a05303-e41c-71db-94d6-b46f69526ef7","effectType":"builtin.gaussian-blur","version":"1.0.0","parameters":{"sigma":12}}
```

- `version` 和 `parameters` 必填。唯一参数 `sigma` 可选，默认 0，范围 `[0,100]`，表示各向同性高斯核的标准差，单位为几何变换前原始输入图像的像素。0 严格等价于不处理。未知参数、版本和越界值必须拒绝。
- 媒体使用原始 stream 尺寸；嵌套使用子序列画布；生成器和调整层使用所属序列画布。预览缩小时，按实际输入宽高与原始宽高的比值分别缩放两轴 sigma，不改写工程参数。
- `sigma` 可为 number 或通用 `numberAnimation`，时钟为 Item 的 `owner` 局部有理秒，支持 linear、hold、cubic-bezier。关键帧和求值结果必须在上述范围内，裁剪/切分复用通用动画规则。
- 在 linear-sRGB 中对预乘 RGB 和 alpha 同时卷积，再恢复 straight RGB；全透明像素的隐藏 RGB 不得泄漏。与颜色效果按堆栈顺序执行。
- 使用可分离离散高斯核：每轴半径 `ceil(3*sigma)`，权重 `exp(-k*k/(2*sigma*sigma))`，归一化后先横向再纵向。输入边界钳位到最近像素，画布不扩张；效果先于 crop/几何变换执行。
- 这是公开 Spec 0.1.0 的新增内置效果，独立效果版本为 1.0.0；不改变旧效果语义，不升级 CLI 或工程版本。内部 0.3/0.4/0.5 工程不能新增该效果，返回 `PROJECT_UPGRADE_REQUIRED`，不得修改版本冒充迁移。归档 Schema 保持不变。

## 3. .cube 文件约定

`.cube` 是 LUT 的一种存储格式，既可以存一维 RGB 曲线，也可以存三维颜色映射；不要把所有 LUT 都称为一维曲线。Cutcraft 0.1.0 的受支持子集为：

- UTF-8 文本，可有 BOM、空行、`#` 注释和一个可选 `TITLE "..."`。
- 必须且只能有一个 `LUT_1D_SIZE N` 或 `LUT_3D_SIZE N`；1D 的 N 为 2…65536，3D 的 N 为 2…256。
- 可选 `DOMAIN_MIN r g b` / `DOMAIN_MAX r g b`，各最多一次，缺省为 0 0 0 / 1 1 1。每通道 max 必须大于 min。头部必须在数据之前。
- 数据行必须为三个有限十进制浮点数。1D 恰有 N 行，三列分别描述各通道的表；3D 恰有 N³ 行，红索引变化最快，其次绿，最后蓝。
- 允许表值超出 `[0,1]`；不允许 NaN/Inf、缺行、多行、重复声明、未知指令，或单文件组合 1D shaper + 3D。遇到不支持的方言应明确拒绝。
- 对每轴 `u=clamp((c-min)/(max-min),0,1)*(N-1)`。1D 线性插值；3D trilinear 采用相邻八角点三线性加权。
- tetrahedral：将小数坐标按降序排序 `fa>=fb>=fc`（相等时 R 优先于 G 优先于 B）；对应单位向量 ea/eb/ec。四顶点为 `v0=(0,0,0)`、`v1=ea`、`v2=ea+eb`、`v3=(1,1,1)`，权重依次为 `1-fa, fa-fb, fb-fc, fc`。在最大边界上以最后一个单元及小数坐标 1 求值。

LUT 文件不是代码，不能执行。导入 MAY 复制到 assets；引用外部文件时必须按项目目录解析相对 URI，且不能修改 LUT 原件。删除被引用的 LUT 默认拒绝；本版 cascade 删除其引用 Item，与其他 Asset 的级联规则一致。自定义效果的资源引用仍由插件声明和验证。

## 4. CLI 0.1.0 渲染约定

源 `stream.video.colorSpace` 标记为 rec709/srgb/linear-srgb；可由可信的媒体原色/传递函数元数据导入，或用 `import --color-space` 显式声明。缺少声明的源不能执行颜色效果。生成器和子序列按序列 Rec.709 编码处理；输出 MP4 显式标记 BT.709 原色、传递函数和矩阵。

采样以像素中心求值，采用预乘 alpha 双线性重建、裁剪边缘钳位，输出保持直通 alpha。CPU 工作精度为 f64，解码及无损中间文件使用 16-bit RGBA；最终 H.264 输出为 8-bit YUV420。
