# Cutcraft Project Specification 0.3.0

本文中的 **MUST**、**MUST NOT**、**SHOULD** 和 **MAY** 表示规范性要求。

## 1. 范围

Cutcraft Project Specification 定义：

- 素材、媒体流、序列、轨道和时间线片段的数据模型；
- 精确时间、轨道叠放、嵌套序列和基础视听属性的语义；
- CLI、GUI 和 Agent 共用的原子领域命令；
- 用于 undo、审计和并发控制的事件记录格式。

本规范不规定具体编解码器、容器、代理文件布局、模型供应商或渲染引擎。MLT、GES、FFmpeg、WebCodecs 或自研引擎都可以成为后端。

## 2. 聚合与实体

`cutcraft.json` 是一个 Project 聚合。聚合内的实体关系如下：

```text
Project
├── Asset map
│   └── Stream[]
└── Sequence map
    ├── Track[]
    │   └── Item[]
    │       └── Effect[]
    ├── Transition[]
    ├── LinkGroup[]
    └── Marker[]
```

### 2.1 Project

Project 是工程的规范快照，包含：

- `specVersion`：规范版本，本版固定为 `0.3.0`；
- `id`：项目稳定 ID；
- `revision`：成功应用一次 operation 后递增一次；
- `assets`：以 Asset ID 为键的素材表；
- `sequences`：以 Sequence ID 为键的序列表；
- `rootSequenceId`：默认预览和导出的入口序列；
- `metadata`：可移植的用户元数据；
- `extensions`：厂商或插件扩展数据。

对象成员顺序没有语义。数组顺序只有在本规范明确说明时才有语义。

### 2.2 Asset 与 Stream

Asset 表示一个外部或受管素材。Asset 本身不是时间线实例；同一素材可以被多个 Item 引用。

Asset 的 `uri` MUST 是相对于工程目录的 URI，或一个合法绝对 URI。实现 SHOULD 优先保存相对 URI。`storage` 的含义：

- `managed`：文件由工程管理，通常位于 `assets/`；
- `external`：工程仅引用外部文件；
- `remote`：素材需要网络解析；
- `generated`：素材由确定性或版本化生成过程产生。

`fingerprint` SHOULD 保存原始素材的 SHA-256。代理、波形、缩略图和索引属于可重建派生数据，MUST NOT 作为 Asset 的唯一事实来源。

一个 Asset 可以包含多个 Stream。Stream 是 Asset 内部的局部实体，其稳定 ID 只需在所属 Asset 内唯一，格式为 `stream_<kind><index>`，如 `stream_v0`、`stream_a0`。支持：

- `video`
- `audio`
- `image`
- `subtitle`
- `data`

同一 Asset 的所有 Stream 共享该素材的呈现坐标系。MediaItem 的 `sourceRange` 在这个坐标系中表达，而不是使用容器或编解码器的原生 PTS。媒体后端 MUST 保留素材内部各 Stream 的原始同步偏移。

### 2.3 Sequence

Sequence 是可播放时间线，也可以被另一个 SequenceItem 嵌套引用。Sequence 引用图 MUST 无环。

Sequence 的 `format` 定义输出画布、帧率和音频采样率。`durationMode`：

- `auto`：序列时长为所有启用 Item 和 Marker 的最大结束时间；
- `fixed`：必须提供 `duration`，超出时长的内容被裁剪，空白区域输出背景色和静音。

### 2.4 Track

Track 是同类型 Item 的容器。支持：

- `video`：视频、图片、生成器、嵌套序列和调整层；
- `audio`：音频、带音频输出的嵌套序列和音频生成器；
- `subtitle`：CaptionItem；
- `data`：不可直接渲染的时序元数据。

`tracks` 数组顺序具有语义：对于视频轨，数组从底层到顶层排列；字幕在普通视频轨之后合成。音频轨同时混音，数组顺序不改变混音结果。

`overlapPolicy`：

- `forbid`：同轨启用 Item 不得重叠；
- `transition-only`：只有被 Transition 连接的相邻 Item 可以在转场语义上重叠；
- `allow`：允许重叠，同轨内后开始的 Item 位于上层；开始时间相同时按 Item ID 升序，后者位于上层。

锁定轨道 `locked: true` 时，修改实现 MUST 拒绝所有会改变其内容或排序的命令，除非调用方显式提供管理员覆盖能力。该能力不属于项目文件。

### 2.5 Item

所有 Item 都具有：

- 稳定 `id`；
- 判别字段 `type`；
- `timelineRange`；
- `enabled`；
- 有序 `effects`；
- 可选 `metadata` 和 `extensions`。

本版定义五种 Item：

#### MediaItem

引用 Asset 中的一个 Stream。时间型流必须提供 `sourceRange`。图片流 MUST 省略 `sourceRange`。

`playback.mode`：

- `fit`：将 `sourceRange` 线性映射到 `timelineRange`，时长不同即产生匀速变速；
- `loop`：在 `timelineRange` 内循环 `sourceRange`，不做拉伸；
- `hold`：保持源范围首帧，MUST NOT 用于音频流。

`reverse` 在映射前反转源时间方向。

#### SequenceItem

引用另一个 Sequence。其 `sourceRange` 是被引用序列内的范围，映射规则与 MediaItem 相同。引用 MUST NOT 直接或间接形成循环。

#### GeneratorItem

由 `generatorType` 和 JSON 参数生成内容。标准内建类型为：

- `builtin.color`
- `builtin.text`
- `builtin.silence`

自定义类型 MUST 使用反向域名，例如 `com.example.lower-third`。

#### CaptionItem

位于 `subtitle` 轨道，保存字幕文本、样式和可选词级时间。词级时间相对于 CaptionItem 的 `timelineRange.start`，且 MUST 完全落在 CaptionItem 范围内。

#### AdjustmentItem

位于 `video` 或 `audio` 轨道，不生成素材，只在自身时间范围内向下方可见轨道应用有序效果。

### 2.6 Transition

Transition 引用同一 Sequence、同一 Track 上的 `fromItemId` 与 `toItemId`。两个 Item MUST 在时间上相邻，且 `from` 的结束时间等于 `to` 的开始时间。

`alignment` 决定转场相对剪辑点的位置：

- `before`
- `center`
- `after`

渲染器从两个 Item 的源素材 handle 中取得额外内容。若 handle 不足，语义校验 MUST 报错，不得静默重复末帧，除非转场参数显式声明这种行为。

标准内建转场：

- `builtin.crossfade`
- `builtin.dip-to-color`

硬切不需要 Transition 实体。

### 2.7 LinkGroup

LinkGroup 将多个 Item 绑定为一次编辑的共同目标，例如一段视频和它的同步音频。LinkGroup 不改变渲染结果，只影响领域命令的默认传播。

- `av`：视音频链接；
- `sync`：保持相对同步偏移；
- `group`：普通选择组。

### 2.8 Marker

Marker 是序列级时序注释。零时长 Marker 表示点，非零时长 Marker 表示范围。Marker 不直接影响渲染。

## 3. 时间模型

### 3.1 Canonical Time

所有具有剪辑语义并持久化到 Project、Operation 或 Event 的时间，MUST 使用两个整数构成的精确有理秒：

```json
{ "num": 1001, "den": 30000 }
```

其值为 `num / den` 秒。这一规则适用于：

- Asset 和 Stream duration；
- Sequence duration；
- Item 的 `timelineRange` 与 `sourceRange`；
- Transition、Marker、Caption word range；
- fade duration、Effect automation 和 keyframe offset；
- Operation 中所有用于移动、裁剪、切分和定位的时间参数。

约束：

- `den` MUST 大于 0；
- 实现 MUST 在写入时用最大公约数约分；
- 零 MUST 写作 `{ "num": 0, "den": 1 }`；
- 时间位置 MUST 非负；
- duration MUST 大于 0，除 Marker duration 可为零；
- 实现进行加减、比较和帧换算时 MUST 使用整数或任意精度有理数，不得用浮点数决定剪辑边界。

JSON number 形式的浮点秒、整数帧号、整数采样号、PTS、DTS，以及 `ticks + timeBase` MUST NOT 作为 canonical 编辑时间写入 Project、Operation 或 Event。厂商扩展也 MUST NOT 使用这些值覆盖或改变 canonical 时间语义。

`createdAt`、`updatedAt`、Operation/Event 的 `timestamp` 是 ISO 8601 墙上时钟，不属于媒体时间，不受本节有理秒格式约束。

### 3.2 时间坐标系

统一表示形式不代表所有值具有同一原点：

- `timelineRange.start` 相对于所属 Sequence 的起点；
- MediaItem 的 `sourceRange.start` 相对于所属 Asset 的呈现起点；
- SequenceItem 的 `sourceRange.start` 相对于被引用 Sequence 的起点；
- Caption word range 和 Effect keyframe offset 相对于所属 Item 的起点。

这些坐标系均使用同一个有理秒 `Time` 类型，跨坐标系只能通过明确的 Item 映射进行转换。

所有 TimeRange 都是左闭右开区间 `[start, start + duration)`。因此两个片段可在同一时间点无缝相邻而不重叠。

CLI MAY 接受 `12.4s`、`120f`、SMPTE timecode 等输入，但写回项目前 MUST 规范化为有理数秒。

### 3.3 媒体后端边界

解封装器和编解码器 MAY 在运行时使用原生 PTS、DTS 和 `time_base`。这些值属于媒体适配器或 `working/` 下的可重建索引，不属于 Project 聚合。

媒体适配器 MUST：

1. 在导入和解码输出时，将原生呈现时间转换为精确有理秒；
2. 在 seek 和解码请求时，将 `sourceRange` 有理秒转换为目标 Stream 的原生 PTS；
3. 只使用 DTS 调度解码或封装，不得让 DTS 影响时间线剪辑边界；
4. 保留同一 Asset 内各 Stream 的同步偏移；
5. 使用确定性的整数有理数换算和明确的采样规则，不得用浮点误差决定选择哪一帧或采样。

## 4. 视频与音频公共属性

### 4.1 坐标

`video.transform.position` 使用归一化目标画布坐标；`anchor` 使用归一化的裁剪后源局部坐标。默认居中，`scale` 中 `1` 表示 contain 适配尺寸，旋转角度为顺时针角度。完整默认值与求值顺序见 [EFFECTS.md](EFFECTS.md)。

裁剪值 `crop.top/right/bottom/left` 是源画面的归一化比例，范围为 `[0, 1]`，相对边之和 MUST 小于 1。

### 4.2 音频

- `gainDb`：分贝增益；
- `pan`：`-1` 为最左、`0` 为居中、`1` 为最右；
- fade duration MUST 不超过 Item duration；
- 当任意轨道 `solo: true` 时，同类未 solo 轨道不输出；
- `muted` 的轨道或 Item 不输出音频。

### 4.3 Effect 与 Automation

Item 的 `effects` 数组按顺序执行。`effectType` 使用 `builtin.*` 或反向域名。参数是 JSON 值；效果实现 MUST 发布额外参数 Schema，项目实现 SHOULD 在媒体校验阶段验证它。

Automation 的 keyframe offset 相对于所属 Item 的起点。Keyframe MUST 按时间递增，且完全位于 Item duration 内。

## 5. 确定性渲染

输出帧 `n` 的采样时间为 `n / frameRate`。一个 Item 在 `start <= t < end` 时处于活动状态。

符合规范的渲染器 MUST：

1. 从 `rootSequenceId` 开始递归求值；
2. 应用轨道 enabled、solo、muted 和 Item enabled 状态；
3. 从底到顶合成视频轨；
4. 按音频时间线混合音频轨；
5. 在视频合成之后绘制字幕；
6. 固定效果和生成器版本；
7. 对相同输入和配置产生语义等价结果。

编码器线程调度可能导致文件哈希不同，因此“确定性”默认指逐帧像素、音频采样和时间语义等价，而不是容器字节完全相同。

## 6. Canonical JSON

写入器 SHOULD 遵循：

- UTF-8，无 BOM；
- 两空格缩进；
- 文件末尾一个换行；
- Map 类型键按 Unicode 码点升序；
- Track 数组保持用户定义顺序；
- Item 数组按 `timelineRange.start`，再按 `id` 升序；
- Effect 数组保持执行顺序；
- 不写入 Schema 中未定义的字段；
- 不依赖 JSON 对象成员顺序。

项目 JSON 不支持注释。解释性文字应放在 `metadata.notes` 或独立 Markdown 文件中。

## 7. 扩展机制

所有通用实体都可以具有 `extensions`。扩展键 MUST 是反向域名，例如：

```json
{
  "extensions": {
    "com.example.review": {
      "status": "approved"
    }
  }
}
```

实现遇到未知扩展时 MUST 保留其原始 JSON，但 MAY 忽略其行为。未知的核心字段 MUST 视为 Schema 错误。

## 8. 领域命令

Operation envelope 包含 `operationId`、`projectId`、`baseRevision`、actor、时间戳和一个 command。

### 8.1 并发

`baseRevision` MUST 等于当前 Project revision，否则命令 MUST 被拒绝。成功应用后 revision 恰好加一；一个 batch 无论包含多少子命令，也只增加一次 revision。

### 8.2 原子性

单个 command 和 `batch` 都 MUST 原子应用。任一语义校验失败时，项目不得发生部分修改。

### 8.3 Put 与 Remove

`*.put` 以稳定 ID 创建或完全替换一个实体，天然幂等。`*.remove` 删除实体；若仍存在引用，默认 MUST 拒绝。实现 MAY 提供显式 cascade 选项，但必须把级联删除的实体记录进 Event。

### 8.4 Item 专用命令

- `item.move`：改变 Track 和/或时间线起点；
- `item.trim`：用完整的新 sourceRange 与 timelineRange 替换旧范围；
- `item.split`：删除原 Item，在剪切点生成调用方指定 ID 的左右两个 Item；
- `item.put`：创建或完整替换 Item。

`ripple` 可为：

- `none`：不移动其他 Item；
- `insert`：将受影响的后续 Item 向后推；
- `remove-gap`：将受影响的后续 Item 向前拉；
- `overwrite`：裁剪或删除目标范围内的其他 Item。

实现 MUST 在 operation 应用前将高层 ripple 语义解析为确定的实体变化，并把完整 before/after 或 inverse commands 写入 Event。

### 8.5 ID 规则

除 Stream 外，持久化实体 ID MUST 使用“类型前缀 + 小写规范 UUIDv7”的格式。UUID 部分必须满足 RFC 9562 的 UUIDv7 版本位和 variant 位；实现 MUST NOT 接受 UUIDv4、无前缀 UUID、大写 UUID 或名称派生 ID。

| 实体 | 格式 |
|---|---|
| Project | `project_<uuidv7>` |
| Asset | `asset_<uuidv7>` |
| Sequence | `seq_<uuidv7>` |
| Track | `track_<uuidv7>` |
| Item | `item_<uuidv7>` |
| Effect | `effect_<uuidv7>` |
| Transition | `transition_<uuidv7>` |
| LinkGroup | `link_<uuidv7>` |
| Marker | `marker_<uuidv7>` |
| Operation | `op_<uuidv7>` |
| Event | `event_<uuidv7>` |
| Actor | `actor_<uuidv7>` |

Asset、Sequence 使用各自 Map 键作为 ID。Track、Item、Transition、LinkGroup、Marker 和 Effect ID MUST 在整个 Project 内全局唯一。Stream ID 是唯一例外：它使用 `stream_v0`、`stream_a0` 等确定性局部 ID，并以 `(assetId, streamId)` 作为完整身份。

ID 创建后 MUST 永久不可变。重命名实体只修改 `name`；复制或拆分实体必须生成新的 UUIDv7。Asset 的内容指纹与身份分离：相同文件可以对应不同 Asset ID，文件内容变化也不得改写 Asset ID。实现 SHOULD 在单进程内维护单调 UUIDv7 生成器，以获得自然的创建顺序和更好的索引局部性。

## 9. Event 与 Undo

每次 operation 应生成 Event。Event 至少记录：

- operation ID；
- 应用前后 revision；
- actor 和 timestamp；
- command；
- `applied` 或 `rejected` 状态；
- 可选 inverse commands；
- diagnostics。

`history.jsonl` 每行是一个完整 Event JSON 对象。Undo SHOULD 作为应用 inverse command 的新 operation，而不是删除历史。

## 10. 校验层级

实现 SHOULD 暴露三个校验层级：

1. **Schema**：JSON 类型、必填字段和枚举合法；
2. **Semantic**：引用存在、ID 唯一、无序列循环、轨道类型匹配、无非法重叠、时间范围合法；
3. **Media**：素材可读取、指纹匹配、stream 存在、sourceRange 未越界、转场 handle 足够、插件和字体可用。

只有通过 Schema 和 Semantic 校验的项目才能写回 canonical `cutcraft.json`。正式渲染前还 MUST 通过 Media 校验。

## 11. 版本演进

读取器 MUST 检查 `specVersion`：

- 不支持的主版本 MUST 拒绝；
- 不支持的次版本 SHOULD 拒绝或通过显式迁移器升级；
- 迁移必须生成新 revision 和可审计 Event；
- 不得静默删除未知扩展。

## 11. Cutcraft 0.3.0 文件与应用契约

### 11.1 主文件与版本

工程唯一快照入口是项目目录的 `cutcraft.json`，而不是 `project.json`。`project.schema.json` 仍是 Schema 文档名称，Project 仍是领域聚合名称。Schema URI 保留历史命名空间 `https://cutstate.dev/schema/v0.3/`，仅用作标识，不依赖联网获取。

- `specVersion` 必填且为 `0.3.0`；不是应用版本，也不是 revision。
- `createdWith`：首次创建此项目的应用描述，不随编辑修改。
- `lastModifiedWith`：最近一次成功修改快照的应用描述。
- `editors`：曾成功创建或修改过项目的应用及版本，按首次使用顺序去重追加。createdWith 和 lastModifiedWith 必须包含在其中。
- 应用描述为 `{id, kind, version}`：id 是稳定产品标识，例如 `cutcraft.cli`；kind 为 cli/desktop/web/library；version 是应用发布版本，例如 `0.1.0`。三者构成去重键。未来客户端沿用相同结构。
- 只读 inspect/validate/render、dry-run、拒绝的命令不更新这些字段。成功修改、undo/redo 都更新 lastModifiedWith 和 editors；编辑器历史不能被 undo 擦除。
- 这些字段必须在计算新快照哈希、写入 history 和 journal **之前**更新，与编辑内容一起提交。禁止修改完 history 后再偷偷补版本字段。

这记录的是修改项目的应用，而不是阅读访问日志；需记录只读访问时应使用独立审计日志。字段表达可追溯性，不是可验证的签名。

### 11.2 项目互斥锁

所有协作实现 MUST 在项目根目录使用同一个 `.cutcraft.lock` 文件的操作系统排他 advisory lock。锁涵盖：读取可修改快照 → 恢复未完成 journal → 验证 revision → 执行修改 → 持久化快照和 history → 清除 journal。读快照也取得该锁，以免读取未恢复的事务。渲染可在取得一致快照后释放锁，输出必须注明该快照 revision。

锁文件是诊断载体，内容示例：

```json
{"pid":12345,"acquiredAt":"2026-09-23T00:00:00Z","application":{"id":"cutcraft.cli","kind":"cli","version":"0.1.0"}}
```

实现 MUST 先取得 OS 锁，再截断和写入 PID 等信息。不能先写 PID 再抢锁。默认非阻塞取得，竞争失败返回 `PROJECT_LOCKED`，不修改任何项目状态；用户可在持锁者完成后重试。

PID、文件存在、时间戳都不是锁是否有效的判据。进程正常退出或崩溃后内核会释放锁；文件可以保留上一次持锁者信息。禁止仅凭 PID 不存在、文件过期或人工删除锁文件来抢锁，禁止持锁期间 rename/unlink/原子替换锁文件，否则不同进程可能锁住不同 inode。锁文件不得用作可删除缓存。

此契约针对支持可靠 advisory lock 和原子文件替换的本地文件系统。云盘同步副本或多个主机各自的本地锁不构成分布式锁；远程协作实现必须通过单一服务端事务协调。外部编辑器若不遵守锁协议，不在互斥保证范围内。

### 11.3 持久化与恢复

`.cutcraft/transaction.json` 是耐久 redo journal，包含待提交完整快照和 history。先同步 journal，再用同目录临时文件同步写入和原子替换 `cutcraft.json`、`history.jsonl`，最后删除 journal 并同步目录。由于两个公开文件不是一次原子替换，所有读取者必须在同一锁内先恢复。journal 不能当作工作缓存删除。

### 11.4 兼容性

0.3.0 是不兼容版本；旧文件必须显式迁移，不能只改文件名或把 specVersion 字符串替换为新版本。迁移需保留原始文件和历史，确认变换锚点、效果参数与新语义一致，填写来源/迁移应用信息，并建立新哈希链，禁止伪造未知的历史客户端版本。当前 CLI 对旧 `project.json` 返回 `LEGACY_PROJECT`，不自动迁移或覆盖；若新旧入口同时存在也拒绝猜测。未支持的协议版本必须拒绝写入。

### 11.5 视觉效果契约

[EFFECTS.md](EFFECTS.md) 定义变换默认值、处理次序、版本化调色、1D 曲线、LUT/.cube 语义。`schemas/effect.schema.json` 对内置参数进行结构验证，且由 Project Schema 直接引用。Semantic 验证曲线横坐标、轨道类型及 LUT 引用。Media 验证 LUT 内容、指纹、色彩空间转换能力、插值匹配及实际效果实现。

## 后续草案（非本版要求）

[0.4.0 动画与转场设计](../0.4.0/ANIMATION.md) 提议以通用内联动画值扩展属性。本文仍描述已发布的 0.3.0，不因该草案而允许现有字段保存动画对象。
