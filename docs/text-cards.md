# 文字卡片

公开 Spec 0.1.0 的标题和字幕可使用同一套可选卡片样式。现有文字命令和无 card 的项目保持原样；内部 0.3/0.4/0.5 不支持 card，不会自动迁移。

```sh
cutcraft -p film.cut item text --track TRACK_ID --start 0 --duration 4 \
  --text 'Connect securely' --font-size 36 --position center --align center \
  --card-width 600 --card-height 240 --card-padding 24 --card-radius 20 \
  --card-gradient-start '#2448bb' --card-gradient-end '#763ba8' \
  --card-border-width 2 --card-border-color '#ffffff80' \
  --card-shadow-color '#00000066' --card-shadow-y 8 --card-shadow-blur 8 \
  --text-stroke-width 1 --text-stroke-color '#000000' \
  --line-height 1.3 --vertical-align center
```

`TRACK_ID` 使用已有视频轨的实际 ID；字幕用 `item caption` 和字幕轨。`--text-file` 可读取含换行的 UTF-8 文本；中文字体可加 `--font-family 'PingFang SC'`，跨机器复现优先使用字体资产。通过 `preview --at`、区间预览或正式 render 检查最终合成。

- `--card-width/--card-height` 必须成对提供，单位为原始序列画布像素。所有像素长度随预览同比缩放。内边距默认16，圆角默认0。
- 背景使用 `--card-color`，或成对的 `--card-gradient-start/--card-gradient-end`。二者互斥。渐变方向 `--card-gradient-direction horizontal|vertical`，默认 horizontal；省略背景为透明。
- `--card-border-width` 启用向内边框，默认白色；`--card-border-color` 可覆盖。
- `--card-shadow-color` 启用卡片外框阴影，x/y 偏移和 `--card-shadow-blur`（高斯 sigma）默认0；偏移允许负数。
- `--text-stroke-width` 启用文字外描边，默认黑色；`--text-stroke-color` 可覆盖。
- `--align left|center|right` 同时控制卡片外框横向定位和每行文字对齐。`--position top|center|bottom` 控制卡片在画布上的纵向位置。`--vertical-align top|center|bottom` 控制文字行框组在内容区的位置，默认 top；`--line-height` 是字号倍率，默认1.2。
- 排版按显式换行，保留空行，最多128行；不自动换行、不缩字、不加省略号，超出内容框裁切。字号、字体与行高共同决定是否放得下。需要另一种字体时明确指定，不自动回退。

宽高、内边距、圆角、边框存在交叉约束，非法配置在编辑时拒绝。可用 `--dry-run` 检查；所有编辑经 Store/Operation，支持 Undo，失败不留下部分改动。卡片样式本身是静态的；整体移动/缩放/透明度动画和现有效果仍可叠加。

持久化位置是标题 `parameters.card` 或字幕 `style.card`。JSON、边界、颜色与透明度规则见 [文字卡片规范](../spec/TEXT_CARDS.md)，`cutcraft schema text-card` 提供离线 Schema；[示例工程](../spec/examples/text-card.cutcraft.json)可直接校验和渲染。此功能不执行 HTML/CSS/SVG。
