# Cutcraft 0.1.0：通用属性动画与转场

**状态：CLI 0.1.0 已接入核心动画、转场与时间映射渲染。** 当前 Schema 位于本目录的 schemas/，内部版本保留在 archive/。具体处理器支持与性能边界见 [实现指南](../docs/animation-and-retime.md)。

## 1. 核心表示：静态值或类型化动画值

允许动画的属性可以保存原来的静态值，也可以保存只有一个 `$animation` 成员的对象。动画直接位于被控制的属性位置，不再额外维护 JSON Pointer、数组下标或第二份默认值。

```json
{
  "video": {
    "transform": {
      "position": {
        "$animation": {
          "version": "1.0.0",
          "valueType": "vector2",
          "timeReference": "owner",
          "keyframes": [
            {
              "offset": {"num": 0, "den": 1},
              "value": {"x": 0.2, "y": 0.5},
              "easing": {"type": "cubic-bezier", "x1": 0.42, "y1": 0, "x2": 0.58, "y2": 1}
            },
            {"offset": {"num": 2, "den": 1}, "value": {"x": 0.8, "y": 0.5}}
          ]
        }
      },
      "scale": {"x": 1, "y": 1},
      "rotationDegrees": {
        "$animation": {
          "version": "1.0.0",
          "valueType": "number",
          "timeReference": "owner",
          "keyframes": [
            {"offset": {"num": 0, "den": 1}, "value": 0},
            {"offset": {"num": 2, "den": 1}, "value": 90}
          ]
        }
      }
    }
  }
}
```

含义：片段内前 2 秒从左移到右，同时顺时针转 90°；位移使用缓入缓出，旋转默认线性。2 秒以后保持最后值。时间是精确有理秒，不是帧编号。

需要单独控制 x 时，position 也可以写为 `{ "x": {"$animation": ...}, "y": 0.5 }`。整向量动画与分量动画二选一；整向量关键帧里的 x/y 必须是静态数值，不允许嵌套动画。

## 2. 通用动画对象

| 字段 | 必填 | 语义 |
|---|---|---|
| version | 是 | 动画表达式版本，当前 `1.0.0`；区别于 specVersion 与效果版本 |
| valueType | 是 | number / vector2 / color / boolean / string / time |
| timeReference | 是 | 当前固定 `owner`，局部时钟由属性所属实体定义 |
| keyframes | 是 | 至少一个关键帧，offset 严格递增且不得重复 |
| extrapolation | 否 | 缺省 `{before:"hold", after:"hold"}`；本版只支持 hold |
| colorSpace | 仅 color | 固定 `linear-srgb`，明确颜色关键帧使用的空间 |

每个关键帧有 `offset`、`value`，以及可选 `easing`。easing 属于**从当前关键帧到下一个关键帧的区间**；最后一个关键帧 MUST 省略 easing。不能以“忽略最后一个 easing”掩盖无效数据。

同一动画的所有 value 必须同类型、同结构；类型由所在属性的 Schema 决定，不由 valueType 自由改变。例如 opacity 只能使用 number 动画，不能传 vector2。关键帧不能再包含 `$animation`。

本版不加入全局动画引用或独立动画 ID；它是所属属性的值对象。属性通过稳定的 Item/Effect/Transition ID 加注册属性名定位，关键帧通过唯一 offset 定位。编辑 offset 属于一次原子更新，受项目 revision 保护。未来如需共享曲线、独立关键帧身份或模板实例，再引入显式引用类型及版本，不能把字符串隐式解释为引用。

## 3. 时钟与采样

属性声明必须同时指定其 owner 时钟和有效时长，不允许实现根据 JSON 层级猜测。

| 使用位置 | owner 的局部时间 τ | 有效时长 D |
|---|---|---|
| Item 的 video/audio 属性 | 序列时间减 Item.timelineRange.start | Item.timelineRange.duration |
| Item.effects 中参数 | 与所属 Item 相同 | 同所属 Item |
| Transition.progress / 参数 | 序列时间减转场窗口起点 | Transition.duration |
| 未来 Sequence 级属性 | 该序列时间 | 序列求值时长 |

offset MUST 是规范化、非负、有界有理秒，满足 `0 <= offset <= D`。末关键帧允许放在 D；Item 本身仍在 `[start,end)` 内有效，边界关键帧提供插值终点，不会额外生成一帧。

- 单关键帧在 owner 整个有效区间保持常量。
- 首关键帧前保持首值，末关键帧后保持末值；属性动画不会延长 Item 可见时间。
- 正好位于关键帧时间时返回该关键帧的值（color 仍需执行 alpha=0 时的透明颜色规范化）。区间为左闭右开，hold 在下一个关键帧时间立即切换到新值。
- 视频在 `t=n/frameRate` 求值；音频可动画属性在对应输出采样时间求值。不先将关键帧时间四舍五入到帧或采样网格。
- 嵌套序列的内部动画在子序列采样时间求值。父 Item 的动画在父 Item 局部时间求值，随后作用于已经求值的子画面。父片段反向播放会反向采样子动画，不会自动反向父片段自身的位移曲线。
- 改变 sourceRange 或素材播放速度本身不改变父 Item 的动画时钟。动画不是绑定在素材原始帧号上的标记。

本版不支持动画专属 repeat/ping-pong、表达式脚本、外部变量、随机数或墙上时钟。未知 timeReference/外推模式 MUST 拒绝，不能退回 hold。未来新增时钟模式不改变当前 `owner` 的含义。

## 4. 插值与缓动

### 4.1 区间算法

对于相邻关键帧 `(t0,v0)` 和 `(t1,v1)`，先用有理数计算 `u=(τ-t0)/(t1-t0)`，再进行浮点数值求值；时间比较始终使用精确有理数。

- `{"type":"hold"}`：区间内返回 v0，到 t1 返回 v1。
- `{"type":"linear"}`：s=u。
- `{"type":"cubic-bezier","x1":...,"y1":...,"x2":...,"y2":...}`：使用端点 (0,0)、(1,1) 和两个控制点。先解 `Bx(q)=u`，再取 `s=By(q)`。**不得直接把 By(u) 当作缓动结果。**

控制点数值必须有限，范围为 `[0,1]`，且 `x1 <= x2`；本版不允许过冲。该约束确保控制点顺序和单调时间轴。参考求值使用 f64 二分求逆，x 误差不大于 `1e-12`，最多 80 次；u=0/1 直接返回 0/1。数值实现允许正常浮点误差，不允许由浮点误差改变关键帧归属。

number 和 vector2 默认 linear；boolean/string 默认且只能为 hold。time 使用有理秒，只接受 linear/cubic-hermite，规则见 [变速协议](RETIME.md)。禁止把 enum/string 自动转成序号插值。

### 4.2 类型规则

- number：`v=v0+(v1-v0)*s`。单位由属性定义。音量 dB 在 dB 值域插值，再转换为振幅；不能悄悄改为线性振幅插值。
- vector2：对 x/y 使用同一个 s 做逐分量插值。路径为直线段，cubic-bezier 只控制沿路径的进度；它不是空间贝塞尔运动路径。
- rotationDegrees 仍是 number：角度不取模、不自动选最短路径。350→10 表示逆时针 340°；350→370 才是顺时针 20°；0→720 表示两整圈。静态渲染的角度归一化必须在动画求值之后进行。
- color：value 为线性 sRGB 的 `{r,g,b,a}`，均在 `[0,1]`。先将 RGB 预乘 alpha，再逐分量插值，最后解除预乘；alpha=0 时 RGB 规范化为 0。持久化仍保存直通 alpha，防止透明颜色产生黑边。色彩属性必须明确接受这一空间才能复用 colorValue。
- boolean/string：区间内保持前值，在下一关键帧处替换；用于未来显式开放的可切换参数。

本动画值协议不定义空间曲线路径、四元数、弹簧、表达式或光流算法；[变速协议](RETIME.md) 单独定义了请求固定光流处理器的接口，不代表算法已实现。它们属于未来类型/插值版本，不能借用当前名字承载另一种算法。

## 5. 哪里允许插入动画

“通用”指复用同一个值协议和求值器，**不是任意 JSON 都自动动画化**。各字段的 Schema 必须显式使用 `StaticValue | AnimationValue`，并声明类型、单位、范围、时钟和插值规则。

本版开放：

| 字段 | 动画类型 | 约束 |
|---|---|---|
| video.transform.position / anchor / scale | vector2，或 x/y 分别 number | 与静态几何含义相同；scale 允许负数/零 |
| video.transform.rotationDegrees | number | 不取模角度 |
| video.transform.crop 四边 | number | 单边 `[0,1]`；求值后的对边之和仍须小于 1 |
| video.opacity | number | `[0,1]` |
| audio.gainDb / pan | number | 沿用静态范围；音频采样时钟 |
| builtin.color-grade 2.0.0 的数值参数 | number | 沿用各参数范围 |
| builtin.lut 2.0.0 的 strength | number | `[0,1]`；LUT 本身保持固定 |
| builtin.gaussian-blur 1.0.0 的 sigma | number | `[0,100]`，原始输入像素；owner 时钟 |
| Transition.progress | number 动画 | `[0,1]`，额外端点规则见第 7 节 |

LUT assetId、插值算法、色彩空间、曲线点数组、文件路径、实体 ID、timelineRange、sourceRange、轨道顺序、specVersion、应用版本、Effect.enabled 和 Item.enabled 本版都**不能动画化**。这些值涉及引用、图结构、媒体调度或算法选择，必须通过编辑命令修改。boolean/string/color 是可复用基础类型，首批核心槽位尚未开放；插件可通过自己的固定版本 Schema 明确采用。

bounded number 的 Schema 必须同时约束静态值和每个关键帧的 value，不能只检查 `valueType:"number"`。跨属性限制不能只看各自关键帧，例如不同时间变化的 crop.left/right：预检需证明有效，或在正式输出所需的全部采样点检查求值结果。违反约束必须失败并保留旧输出，不能静默裁剪、钳位或用缺省值替代。

未知插件的 opaque parameters/metadata/extensions 内即使出现 `$animation`，通用读取器也 MUST 原样保留，不能自行执行或重写。只有插件声明的动画槽位才进入求值。

## 6. 效果参数复用

```json
{
  "effectType": "builtin.lut",
  "version": "2.0.0",
  "parameters": {
    "assetId": "asset_01a05303-e41c-71db-94d6-b46f69526ef9",
    "inputColorSpace": "rec709",
    "outputColorSpace": "rec709",
    "interpolation": "tetrahedral",
    "strength": {
      "$animation": {
        "version": "1.0.0",
        "valueType": "number",
        "timeReference": "owner",
        "keyframes": [
          {"offset": {"num": 0, "den": 1}, "value": 0},
          {"offset": {"num": 2, "den": 1}, "value": 0.8}
        ]
      }
    }
  }
}
```

这是 Effect 片段，完整 Effect 仍需要 id。先求值 strength，得到普通数值参数，再执行 LUT；其他调色参数同理。version 1.0.0 继续仅接受静态参数；2.0.0 允许声明过的动画槽位。`builtin.curves` 的点数组仍为静态 1.0.0，不暗中引入曲线形状动画。

旧 `Effect.automation` 与内联动画不能并存；本版核心 Effect Schema 不允许旧 automation 字段。迁移必须按旧插件的参数绑定规则显式转换；无法确定的 parameter/easing 字符串必须报告，不能猜测。空 automation 可删除。

## 7. 转场窗口、进度与合成

### 7.1 JSON 表示

```json
{
  "id": "transition_01a05303-e41c-71db-94d6-b46f69526ef5",
  "fromItemId": "item_01a05303-e41c-71db-94d6-b46f69526ef1",
  "toItemId": "item_01a05303-e41c-71db-94d6-b46f69526ef4",
  "transitionType": "builtin.crossfade",
  "version": "2.0.0",
  "duration": {"num": 1, "den": 1},
  "alignment": "center",
  "progress": {
    "$animation": {
      "version": "1.0.0",
      "valueType": "number",
      "timeReference": "owner",
      "keyframes": [
        {"offset": {"num": 0, "den": 1}, "value": 0,
         "easing": {"type": "cubic-bezier", "x1": 0.42, "y1": 0, "x2": 0.58, "y2": 1}},
        {"offset": {"num": 1, "den": 1}, "value": 1}
      ]
    }
  }
}
```

两个 Item 在同一视频轨相邻，剪辑点为 C，时长为 D：before 窗口 `[C-D,C)`，center 为 `[C-D/2,C+D/2)`，after 为 `[C,C+D)`。D/2 保留精确有理数，不强制取整到帧。窗口 MUST 在序列有效范围内，且不能越过这两个 Item 的外侧时间线边界；同轨转场窗口不能互相重叠。

省略 progress 时，使用 `p=(t-windowStart)/D`。显式 progress 必须为 number 动画，第一帧 offset=0/value=0、末帧 offset=D/value=1；中间值在 `[0,1]`。允许用户显式让进度中途回退或停顿，不能强制重新排序 value；offset 仍严格递增。

进度只控制混合，不改变两个源的采样时钟。不是把 B 从零开始拉伸填满 D。源画面沿各自既有 sourceRange/播放映射继续取样，因此转场窗口可能需要 A 的尾部 handle 或 B 的头部 handle；reverse/fit 必须纳入计算。图像和常量生成器可延展；时间相关生成器必须声明窗口外求值能力。素材余量不足必须拒绝，不能重复最后一帧冒充转场。

### 7.2 内置转场 2.0.0

两个输入先分别完成源采样、颜色效果、几何变换及 Item opacity，再以同一画布尺寸进入转场。在转场期间输出替代这两个 Item 的常规绘制，不能再叠画一次。之后再与更低轨道按图层规则合成。

令 A/B 为**线性 sRGB、预乘 alpha**的 RGBA 值：

- `builtin.crossfade`：`R=(1-p)*A+p*B`。
- `builtin.dip-to-color`：parameters.color 必填，为直通 alpha 的线性 sRGB RGBA。先预乘得到 K；p≤0.5 时 `R=(1-2p)*A+2p*K`，p>0.5 时 `R=(2-2p)*K+(2p-1)*B`。p=0.5 必须恰好等于 K。

取样源的 Item 动画在相应 Item 局部时间求值；进入源 handle 区域时使用该动画的端点保持规则。最终颜色在解除预乘后进入后续约定的合成/编码流程。音频不因为视频 Transition 而自动淡化，本版这两个内置转场只允许视频轨；音频需独立 gainDb 动画或后续专用音频转场。

旧 1.0.0 没有完整颜色/进度契约；本版将这两个内置转场固定为 2.0.0，迁移须核对旧实现的结果，不能仅改版本字符串。

## 8. 编辑、撤销与可扩展性

- 保存动画仍属于同一 Item/Transition 聚合。现有 item.put / sequence.put / transition.put 可以整体替换相应值，不需要暴露任意 JSON Patch。
- 移动 Item 只移动 timelineRange.start，keyframe offset 保持不变，动画随片段移动。
- 修改 sourceRange 不自动修改关键帧；修改 Item duration 也不隐式缩放关键帧。若新时长导致越界，整次操作拒绝，调用者通过原子 batch 同时修改动画。
- split 必须裁切两段局部函数：右半 offset 减去分割时间，在边界补充求值点；跨越的 cubic-bezier 区间必须重新参数化，不能只平移关键帧。拆分后两侧求值需在声明的浮点精度内等价。未实现保真拆分时必须明确拒绝。
- 用户显式选择“整体拉伸动画”时，所有 offset 按新旧时长的精确有理比缩放，归一化 easing 控制点不变。这是不同于 trim 的编辑意图。
- 原子性、revision、锁、history、inverseCommands、未知字段保留规则沿用项目协议。撤销恢复表达式，不保存每帧求值后的快照。
- 将来开放新属性只需声明类型/时钟/范围并引用 animation Schema；如果改变值类型、求值方式或资源依赖，需要升级对应协议/插件版本。

## 9. 文件与符合性

- `schemas/animation.schema.json`：独立可复用动画值 Schema。
- `schemas/common.schema.json`：在 transform、opacity、gainDb、pan 开放动画。
- `schemas/effect.schema.json`：静态 1.0.0 与可动画参数 2.0.0 并存。
- `schemas/project.schema.json`：完整项目和 Transition.progress。
- `examples/animated.cutcraft.json`：完整位移、旋转、透明度与曝光动画项目。
- `examples/transition.cutcraft.json`：完整带缓动转场项目。
- Rust CLI `validate` 与 `tests/animation.rs`：Schema、语义及反例验证；媒体校验仍需可读取的实际素材。

读取器只有在支持对应动画版本、属性绑定和效果版本时才能渲染。不支持本协议版本的 CLI 应拒绝整个项目；当前 CLI 在注册槽位求值，不能把动画对象转换成 0 或丢掉动画后输出静态画面。

## 10. 时间值与变速扩展

[RETIME.md](RETIME.md) 定义了 time 类型和 playback.timeMap 绑定。time 的 value 为非负、规范化有理秒，静态数值动画的 cubic-bezier 不能用于选择源帧；时间值使用精确的线性或 Hermite 多项式。timeMap 额外要求覆盖整个片段、有至少两个关键帧并在源窗口内。转场 handle 的外推是单独的素材访问请求，不使用通用动画的区间外 hold 代替。
