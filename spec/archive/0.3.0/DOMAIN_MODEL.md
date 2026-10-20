# Cutcraft 领域模型

## 聚合边界

`Project` 是唯一聚合根。所有修改必须在 Project revision 上原子提交。Asset 和 Sequence 是项目内一级实体；其余实体由 Sequence 或 Item 拥有。

| 类型 | 身份 | 所属对象 | 生命周期 |
|---|---|---|---|
| Project | `project.id` | 无 | 工程目录 |
| Asset | `assets` Map key | Project | 可被任意 Item 引用 |
| Stream | `stream.id` | Asset | 随 Asset 存在 |
| Sequence | `sequences` Map key | Project | 可作为根序列或嵌套序列 |
| Track | `track.id` | Sequence | 随 Sequence 存在 |
| Item | `item.id` | Track | 一个可编辑时间线实例 |
| Effect | `effect.id` | Item | 有序执行 |
| Transition | `transition.id` | Sequence | 连接同轨相邻 Item |
| LinkGroup | `link.id` | Sequence | 绑定多个 Item 的编辑行为 |
| Marker | `marker.id` | Sequence | 不影响渲染的时序注释 |

Asset/Sequence 的 Map key 是身份，不在值中重复存储。除 Stream 外，实体使用带类型前缀的 UUIDv7，创建后不可修改。Stream 使用 `stream_v0`、`stream_a0` 等 Asset 内局部 ID；Track、Item、Effect、Transition、LinkGroup 和 Marker ID 在整个 Project 内全局唯一。

## 实体和值对象

### 实体

实体具有稳定 ID，允许在 revision 之间追踪：

- Asset
- Sequence
- Track
- Item
- Effect
- Transition
- LinkGroup
- Marker

### 值对象

值对象按值相等，没有独立生命周期：

- Rational / Time
- TimeRange
- SequenceFormat
- Transform
- VideoProperties
- AudioProperties
- Color
- Playback
- CaptionStyle
- Automation / Keyframe
- Fingerprint
- Actor
- Diagnostic

## Asset 与 Item 的分离

Asset 表示“素材是什么”，Item 表示“素材在某条时间线里如何使用”。因此：

```text
Asset(camera.mov)
├── Stream(stream_v0)
└── Stream(stream_a0)

Sequence.main
├── Track.video
│   └── MediaItem(asset=camera.mov, stream=stream_v0, source=12s..18s)
└── Track.audio
    └── MediaItem(asset=camera.mov, stream=stream_a0, source=12s..18s)
```

视音频 Item 通过 LinkGroup 保持同步。删除一个 Asset 时，只要仍有 Item 引用它，领域层必须拒绝操作或执行显式、可审计的级联删除。

## Item 类型

| Item | 引用/生成内容 | 合法轨道 |
|---|---|---|
| MediaItem | Asset 中的一个 Stream | 由 Stream kind 决定 |
| SequenceItem | 另一个 Sequence | video、audio |
| GeneratorItem | 内建或插件生成器 | video、audio、data，取决于生成器 |
| CaptionItem | 文本和词级时间 | subtitle |
| AdjustmentItem | 作用于下层的 Effect 链 | video、audio |

所有 Item 使用显式 `timelineRange.start`，数组下标不表示时间。数组仅按时间和 ID 做 canonical 排序。

## 时间映射

MediaItem 和 SequenceItem 同时具有两个空间：

```text
sourceRange   --素材/子序列中的范围
timelineRange --父序列中的范围
```

两者都使用 `{num, den}` 精确有理秒，但原点不同：`sourceRange` 位于 Asset 或被引用 Sequence 的呈现坐标系，`timelineRange` 位于父 Sequence 坐标系。同一 Asset 的 Stream 共享素材呈现原点，媒体后端负责保留其原始同步偏移。

`playback.mode = fit` 时，两段时长不同会产生线性变速；`loop` 重复 sourceRange；`hold` 保持首帧。所有范围均为左闭右开。

## 媒体后端边界

Project 聚合只包含 canonical 有理秒。原生 PTS、DTS、`time_base`、关键帧位置和文件偏移不是领域状态，只能存在于内存或 `working/` 下的可重建媒体索引。

```text
Project sourceRange ({num, den} 秒)
        │
        ▼
MediaResolver（精确整数换算）
        │
        ▼
Native Stream PTS / time_base
```

解码输出的 PTS 用于映射到素材呈现时间；DTS 只用于底层解码和封装调度。任何原生媒体时间都不得反向成为 Timeline 的权威剪辑边界。

## 核心不变量

一个 Project 在提交前必须满足：

1. `rootSequenceId` 存在；
2. 所有引用目标存在；
3. 所有全局实体 ID 都是类型正确、不可变且唯一的 UUIDv7；
4. 嵌套 Sequence 图无环；
5. Item 类型和 Track kind 匹配；
6. 所有持久化编辑时间都是已约分且非负的 `{num, den}` 有理秒；
7. duration 大于零，Marker 除外；
8. `forbid` 和 `transition-only` 轨道不存在真实 Item 重叠；
9. Transition 两端属于同序列、同轨道且时间相邻；
10. Caption 词级范围和 Effect keyframe 不越过 Item duration；
11. MediaItem 的 sourceRange 不越过 Stream duration；
12. Project、Operation 和 Event 不使用 PTS、DTS、整数帧、整数采样或 `ticks + timeBase` 表达剪辑时间；
13. 每个成功 Operation 只让 Project revision 加一。

## 领域服务

这些行为不属于单个实体，建议由领域服务实现：

- `ProjectValidator`：Schema、Semantic、Media 三层校验；
- `OperationApplier`：revision 检查、命令执行和原子提交；
- `RippleResolver`：把 ripple 意图解析成确定的 Item 变化；
- `TimelineEvaluator`：求值嵌套序列、轨道状态和活动 Item；
- `RenderPlanner`：将有效时间线编译到具体渲染后端；
- `MediaResolver`：解析 URI、指纹、Stream 和代理；
- `CanonicalWriter`：稳定排序、时间约分和规范 JSON 输出；
- `HistoryService`：记录 Event、生成 inverse command 和执行 undo。

## 命令与事件

Command 表示意图，Event 表示结果：

```text
OperationEnvelope
  command
     │
     ▼
OperationApplier ── validate ── mutate copy ── commit
     │
     ▼
HistoryEvent(applied/rejected, before/after revision, inverse commands)
```

CLI、GUI 和 Agent 必须走同一个 OperationApplier，不允许其中任一入口直接修改 JSON 文件。这样可以保证锁定、链接、ripple、revision 和 undo 的语义一致。
