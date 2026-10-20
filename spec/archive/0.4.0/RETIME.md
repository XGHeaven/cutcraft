# Cutcraft 0.4.0 草案：变速与时间重映射

CLI 0.4.0 已实现本协议的精确时间映射、frame-hold/frame-blend 与固定线性 PCM 重采样处理器。光流与保音高仍为处理器接口，未安装实现时明确拒绝。使用方式与具体边界见 [实现指南](../../../docs/animation-and-retime.md)。

## 1. 唯一时间映射

令片段局部时间为 τ，时间线时长为 T，sourceRange.start 为 S，sourceRange.duration 为 L。

```text
序列输出时间 t → τ=t-Item.start → 源偏移 f(τ) → 素材时间 S+f(τ)
```

瞬时速度是 `df/dτ`：1 为正常，2 为两倍速，0.5 为半速，0 为定格，负数为倒放。这里是“源秒 / 输出秒”，不是 FFmpeg setpts 的倍率（后者匀速情况下为其倒数）。

不再额外持久化一份可独立修改的 speed 曲线。UI 可显示和编辑速度，但必须编译为下面的时间映射；速度曲线是 f 的导数，积分结果不能与另一份 sourceTime 冲突。

## 2. 匀速：继续使用 fit

```json
{
  "timelineRange": {"start":{"num":0,"den":1}, "duration":{"num":6,"den":1}},
  "sourceRange": {"start":{"num":10,"den":1}, "duration":{"num":3,"den":1}},
  "playback": {"mode":"fit", "reverse":false}
}
```

正向 `f(τ)=L*τ/T`，速度 `L/T=0.5`。reverse=true 时 `f(τ)=L-L*τ/T`，速度为 -0.5。3 秒源素材播放 6 秒；源区间仍是 `[10,13)`。

sourceRange 与 timelineRange 已经完整确定速度，不新增冗余 `rate` 字段。拟议的 `item speed --rate 2` 是便捷编辑命令，不是新增持久化属性：固定源范围，计算 `T=L/abs(rate)`，负值设置倒放；rate=0 不能用于推导无限时长，应使用显式定格/重映射区间。

## 3. 非匀速：playback.mode=remap

```json
{
  "timelineRange": {"start":{"num":0,"den":1}, "duration":{"num":6,"den":1}},
  "sourceRange": {"start":{"num":10,"den":1}, "duration":{"num":6,"den":1}},
  "playback": {
    "mode":"remap",
    "version":"1.0.0",
    "timeMap": {
      "$animation": {
        "version":"1.0.0",
        "valueType":"time",
        "timeReference":"owner",
        "keyframes":[
          {"offset":{"num":0,"den":1}, "value":{"num":0,"den":1}},
          {"offset":{"num":2,"den":1}, "value":{"num":1,"den":1}},
          {"offset":{"num":4,"den":1}, "value":{"num":5,"den":1}},
          {"offset":{"num":6,"den":1}, "value":{"num":6,"den":1}}
        ]
      }
    }
  }
}
```

前两秒 0.5 倍速、中间两秒 2 倍速、最后两秒 0.5 倍速。keyframe.offset 是输出局部时间，value 是相对于 sourceRange.start 的源偏移，不是绝对素材时间。

规则：

- timeMap 复用 `$animation` 外壳，增加 `valueType:"time"`，value 使用精确有理秒。第一点 offset 必须为 0，最后一点 offset 必须为 T，至少两个点。
- 所有 offset 严格递增。所有 value 在 `[0,L]`，不要求递增，也不要求从 0 到 L；sourceRange 是允许访问的源窗口，不必全部播放。
- remap MUST 省略 reverse。倒放由下降的源偏移表示，避免双重反转。
- 默认线性插值。相同 value 表示定格；下降表示倒放；速度突变允许，但源时间必须连续。不允许用 hold 从某个源时间直接跳到另一个源时间；跳剪用两个 Item 表达。
- 图片没有 sourceRange，不允许 remap；它的移动/缩放使用普通属性动画。MediaItem 的时间型流和 SequenceItem 可以使用 remap。
- 不能嵌套 timeMap，不能用素材采样时间反过来求 timeMap，也不能把 timelineRange 本身动画化。

例如源偏移 `0 → 2 → 2 → 0`，对应先正放、定格、倒回。时间点可任意使用有理秒，不要求作者先对齐到输出帧。

## 4. 平滑变速：精确的 Hermite 时间曲线

通用数值动画的 cubic-bezier 需要求逆，容易把求值误差带入源帧选择。时间值单独提供 **cubic-hermite**：关键帧表示时间位置，区间起止斜率表示速度，所有计算可保持有理数。

```json
{
  "offset":{"num":0,"den":1},
  "value":{"num":0,"den":1},
  "easing":{
    "type":"cubic-hermite",
    "outRate":{"num":1,"den":2},
    "inRate":{"num":3,"den":2}
  }
}
```

这个 easing 属于当前点到下一点的整个区间：outRate 是区间起点的速度，inRate 是区间终点的速度（不是当前点的入速度）。rate 是有符号、规范化有理数，无单位。最后一个关键帧不带 easing。

设下一点为 `(offset=2, value=2)`，则上例从 0.5 倍速平滑升到 1.5 倍速，2 秒恰好前进 2 源秒。在局部时间 1 秒，源偏移为 0.75 秒；不是简单取两个源位置的中点。

对于相邻 `(t0,s0)`、`(t1,s1)`，`d=t1-t0`，`u=(τ-t0)/d`，区间端点速度为 m0/m1：

```text
f(u) = (2u³-3u²+1)s0 + (u³-2u²+u)d*m0
     + (-2u³+3u²)s1 + (u³-u²)d*m1
```

**时间映射禁止 cubic-bezier 和任意浮点时间值。** timeValue 在普通动画中也只接受 linear/cubic-hermite；它不复用 number 的缓动类型。非时间属性若未来采用 Hermite，须另行声明单位和 Schema。

为使素材边界可验证，规定该区间等价的四个 Bézier 纵向控制值均在 `[0,L]`：`s0, s0+d*m0/3, s1-d*m1/3, s1`。这是一项保守的合法性约束：不尝试接受“控制点越界但实际曲线未越界”的特例。点值合法但控制值越界的曲线也必须拒绝；禁止静默夹到源边界。

- 起止速度可以为负，可在区间内部经过零并改变方向。
- s0=s1 且 m0/m1 都为 0 才是完整定格区间；同端点、非零速度可能是先出去再回来。
- 相邻区间源位置连续，但速度不强制连续。若 UI 声称“平滑连接”，需令相邻端点速度相同；自动平滑必须把最终数值写入 JSON，不在渲染时偷偷重新拟合。
- 拖动一个速度控制点不会自动改变终点位置或时长；二者是约束，其他区间的速度可能随曲线形状改变。编辑器必须展示平均速度 `(s1-s0)/d` 与局部端点速度的区别。

## 5. 源边界与画面采样

时间映射是连续源坐标，不是帧编号。每个输出时刻单独计算 f，不能逐帧累加浮点速度。

sourceRange 仍为左闭右开。f=L 是合法的边界锚点，但不得读取范围外一帧：按现有 exclusive-upper 规则选择该边界之前的帧/采样；f=0 使用下界。内部边界使用映射朝向：局部斜率为负时采用左侧极限，否则采用右侧；斜率恰为零时，取向右第一非零导数决定进入方向，完整常量区间则使用普通帧覆盖规则。嵌套映射需传播边界方向，不能依赖添加浮点 epsilon。

`playback.videoSampling` 与变速映射分离：

- `{"method":"frame-hold"}`，默认：选覆盖映射时刻的源帧；慢动作重复帧、快动作跳过帧。没有光流或合成新内容。
- `{"method":"frame-blend"}`：在包围映射坐标的两个源 PTS 之间按时间距离混合，使用线性颜色/预乘 alpha；边界钳到源窗口内可用帧，不读取窗外样本。
- `{"method":"optical-flow","processor":{"id":"com.example.flow","version":"1.0.0","parameters":{}}}`：请求固定实现的光流插帧；算法/模型与配置必须可复现且可获得。后端不支持时拒绝，不能自动改成 frame-hold。模型型插件应在 parameters 中固定模型内容指纹和随机种子（如适用）。

frame-hold/blend 指的是时间采样，独立于图像空间缩放的双线性插值。变速不改变序列输出帧率。素材解码索引可支持 VFR；这不代表当前 CFR 后端已经支持 VFR 或非网格边界。

## 6. 音频政策

视频与音频是分开的 Item。AV LinkGroup 只表达编辑联动，不能隐式复制时间映射。变速操作必须原子更新组内所有成员，按各自源起点解析相同的源偏移函数，保持同步；无法找到等价映射则拒绝。

对任何不是恒定 +1 倍速的音频 Item，必须显式设置 `playback.audioPolicy`：

```json
{
  "mode":"preserve-pitch",
  "processor":{"id":"com.example.time-stretch","version":"1.0.0","parameters":{}}
}
```

- `resample`：重采样，音高随速度改变；负速率输出倒放声音。
- `preserve-pitch`：时间拉伸，尽量保持音高；本版只允许正速率区间或完整零速定格区间。带倒放或未支持速度范围的请求必须拒绝。
- `mute`：输出静音，适合变速蒙太奇；不要求处理器。

resample/preserve-pitch 都要求固定 processor 描述，便于锁定插值、抗混叠或拉伸算法及参数。这里只定义政策和接口，不宣称某个具体处理器已经安装或实现。+1 倍速且没有处理政策时，沿用普通音频播放。

完整零速区间输出静音，不重复单个音频采样形成直流，也不隐式制造循环声。非零区间靠近零速时仍按处理器支持范围预检；不可无提示钳速度。孤立零导数不是定格区间。方向切换或速度突变可能产生音频瞬态，协议不暗中加淡化；需要的交叉淡化/音量动画须显式记录。

## 7. 与属性动画、转场的关系

- timeMap 使用父 Item 的 owner 时间。父 Item 的位移、旋转、效果强度也使用父局部时间；不会因为素材倒放而倒着执行这些动画。
- SequenceItem 的 timeMap 控制子序列采样时间；子序列内的动画随采样时间加速、减速或倒放。这提供了“把画面及其内部动画一起变速”的方式。
- 转场 progress 使用转场局部时钟，仅控制混合，不再改变源采样速度。A/B 各按自己的映射采样。
- 转场进入 remap Item 的 handle 区间时，用最近端点的源位置与单侧速度作线性外推（不是任意延伸三次多项式）。外推只能访问 sourceRange 外但 Asset/Sequence 内真实可用的 handle；映射定义域内仍不得越出 sourceRange。
- 零端点速度表示保持边界采样；未声明定格能力的时间型生成器不得据此假设可无限延长。所有转场所需源范围须在发布输出之前验证。

## 8. 编辑语义与拟议 CLI

以下命令已在 CLI 0.4.0 实现：

```sh
cutcraft item speed ITEM --rate 2 --ripple none
cutcraft item speed ITEM --rate -0.5 --ripple remove-gap
cutcraft item time-map ITEM --file mapping.json
```

- 匀速 speed 命令固定 sourceRange，修改 timelineRange.duration，负号设置 fit.reverse；remap Item 必须显式选择“替换为匀速”，不能悄悄丢掉原曲线。
- 修改时长默认保持左端起点，不自动改后续片段位置；发生不允许的重叠则整次拒绝。缩短是否留空、延长是否推后由显式 ripple 策略决定，遵循领域命令的统一语义。
- 拟议编辑命令还需显式选择属性动画的处理：`keep-local` 保留关键帧局部时间，越界拒绝；`stretch` 把 offset 乘以新旧时长比，rotation 等属性值不变。不能靠换素材时长偷偷拉伸动画。
- 对 remap 做整体时间拉伸 q 倍时：keyframe.offset 乘 q，source value 不变，Hermite outRate/inRate **除以 q**。只缩放关键帧时间、不调整切线速度会改变原来的映射形状。
- 移动片段不改变映射；改 sourceRange.start 会平移全部绝对源采样，timeMap.value 保持相对偏移。
- trim/split 需限制原 f 到保留区间、平移局部 offset 并补边界点；Hermite 以边界值和导数重建，可用有理数精确完成。sourceRange 可以继续保留原窗口，不强制收紧。不能像匀速片段一样只按新旧时长比例猜测 sourceRange。
- 所有编辑仍走原子事务、锁、revision、undo。效果不可用、越界或无法保真编辑时拒绝，不能输出部分修改。

## 9. 与现有模式的边界

fit/loop/hold 保留既有定义；loop 与 remap 互斥。需要“往返/循环并变速”时显式写出时间映射，或使用嵌套序列。hold 是视频固定采样的便捷模式，不接受 reverse；audio 禁用 hold，音频的零速区间通过 remap 与明确 audioPolicy 表示。

0.4.0 playback 改为判别联合：fit/loop 可带 reverse，hold 不可带 reverse，remap 必须带 version/timeMap 且不可带 reverse。静态省略 playback 仍等价于 fit 正向。旧含 reverse:false 的 hold 迁移时需显式删除该无意义字段。时间映射只在明确的 playback.timeMap 槽位执行；opaque 插件 JSON 中相同结构不自动成为时间调度指令。
