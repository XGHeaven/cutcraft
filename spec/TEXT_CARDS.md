# 文字卡片（公开 Spec 0.1.0）

`builtin.text` 1.0.0 的 `parameters.card` 与字幕的 `style.card` 为可选对象。省略时保持已有文字语义；内部 0.3/0.4/0.5 不支持 card，明确拒绝，不改写工程。此兼容扩展登记为 0.1.0-r3，不改变 specVersion、事件、生成器版本或既有 ID。

```json
{"widthPixels":600,"heightPixels":240,"paddingPixels":24,"radiusPixels":20,
 "background":{"type":"linear-gradient","startColor":{"r":0.1,"g":0.2,"b":0.8,"a":1},"endColor":{"r":0.4,"g":0.1,"b":0.6,"a":1},"direction":"horizontal"},
 "border":{"widthPixels":2,"color":{"r":1,"g":1,"b":1,"a":0.5}},
 "shadow":{"offsetXPixels":0,"offsetYPixels":8,"sigmaPixels":8,"color":{"r":0,"g":0,"b":0,"a":0.4}},
 "textStroke":{"widthPixels":1,"color":{"r":0,"g":0,"b":0,"a":1}},
 "lineHeight":1.3,"verticalAlign":"center"}
```

- 宽高必填，单位为所属序列原始画布像素，范围 (0,4096]；内边距默认 16、圆角默认 0，范围 [0,2048]。内边距必须小于宽高各一半；圆角不大于短边一半。预览时所有像素长度随所属序列同比缩小，输出至少一像素。
- 卡片外框沿用 `align`（默认 center）和 `position`（默认 bottom）在序列画布定位：左右/顶边留 20 原始像素，底边留 40；中心对齐为画布居中。卡片可超出画布，最终裁切。
- `background` 可省略（透明）；solid 必须给 color；linear-gradient 必须给 startColor/endColor，direction 可选 horizontal（默认）/vertical。颜色均是线性 sRGB straight RGBA [0,1]。渐变在外框两端之间按预乘 RGBA 插值；圆角遮罩与透明度只乘一次。
- `border` 可选，widthPixels 必填 (0,256]、color 必填，宽度不大于短边一半；边框向内绘制，在背景之上、文字之下。外轮廓固定，内圆角为 max(0,radius-width)。
- `shadow` 可选，color 必填；offsetXPixels/offsetYPixels 默认 0、范围 [-4096,4096]；sigmaPixels 默认 0、范围 [0,100]。阴影来自完整外框遮罩（不受背景透明度影响），先偏移，再按高斯 sigma 模糊，放于卡片之下；超出序列画布裁切。模糊边界用透明像素扩展。
- `textStroke` 可选，widthPixels 必填 (0,32]、color 必填。描边先于填色，颜色在线性光中合成；预览时宽度缩放并四舍五入到整数像素。
- `lineHeight` 默认 1.2、范围 [0.5,4]，表示字体像素大小的倍率。`verticalAlign` 为 top（默认）/center/bottom，控制整个行框组在扣除 padding 后的内容框中的位置。每行高度为 fontSizePixels×lineHeight；字号默认48，卡片内字号上限4096。
- 内容按显式 LF 换行（CRLF 规范化成 LF），保留空行，最多128行；不自动换行、不省略文字。每行按 align 左/中/右对齐；字体度量定位行顶。超出矩形内容框的字形/描边裁切。卡片内禁止 tab 等非换行控制字符，避免平台排版差异。
- card 与字幕旧有 backgroundColor/maxLines 不可同时使用，避免两套背景/截断语义。未知字段、非法组合或数值必须拒绝。card 参数不注册动画；整体动画仍使用 Item transform/opacity，效果应用于完整卡片。
- 字体解析、字体资产优先、TTC 选面、缺字报错仍遵守 SPEC 字体规则；不引入 HTML/CSS/SVG 执行环境。生成器共享 Schema 可描述 card，但领域校验按集中能力集合拒绝旧工程。
- `item.put`/batch/Undo 原样保留 card；编辑失败原子回滚。新增 CLI 参数只是构建同一对象，CLI 与其他客户端复用领域验证。
