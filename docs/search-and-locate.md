# 搜索素材与定位片段

本页的搜索和详情命令查询项目当前快照，返回 JSON、`revision` 和实际 ID，不修改项目或历史，不启动 FFmpeg。列表返回 `count` 和对应的 `assets`、`items`、`tracks`、`sequences` 或 `usages` 数组；没有匹配时返回空数组。详情和 ID 筛选需要完整 ID，不存在则报错，不会把拼错的 ID 当作空搜索结果。

## 素材搜索

```sh
cutcraft -p demo.cut asset list
cutcraft -p demo.cut asset search '喜字'
cutcraft -p demo.cut asset search --name 'IMG_' --path '云南/婚礼'
cutcraft -p demo.cut asset list --kind image
cutcraft -p demo.cut asset list --tag wedding
cutcraft -p demo.cut asset list --unused
cutcraft -p demo.cut asset info ASSET_ID
```

`asset search` 是 `asset list` 的别名。可选位置参数搜索素材 ID、名称、URI、解析后的本地路径和标签；`--name`、`--path` 限定匹配字段。字符串匹配忽略大小写，采用字面子串，不解释正则、通配符或自然语言。多个条件取交集；`--kind` 精确匹配素材类型，`--tag` 忽略大小写但匹配完整标签。名称相同的素材全部返回，由 ID、路径、流信息区分。

路径搜索同时检查存储的 URI 和解析后的绝对本地路径，自动解码 URI 中的中文、空格，无需知道 `%E4%...` 编码。文件即使丢失也可以搜索。远程 URI 仍可按 URI 搜索，但返回的本地 `path` 为 `null`。`availability` 是快照里的登记状态；查询不会检测磁盘实时状态，需要时运行 `validate --level media`。

Windows 下路径查询接受 `/` 和 `\` 两种分隔符，包括通过素材路径搜索片段；Unix 下反斜杠仍按文件名中的字面字符处理。返回的 `path` 保持系统原生格式。

列表提供 `assetId`、名称、路径、类型、存储方式、标签、`streams` 和 `usageCount`。`asset info` 额外返回完整 `asset` 对象，可据此保留字段并构造 `asset.put`。

## 素材替换

```sh
cutcraft -p demo.cut asset replace ASSET_ID /path/to/new-audio.wav
cutcraft -p demo.cut asset replace ASSET_ID /path/to/new-video.mp4 --managed --color-space rec709
cutcraft -p demo.cut asset replace ASSET_ID /path/to/new-video.mp4 --dry-run
cutcraft -p demo.cut undo
```

`asset replace ASSET_ID FILE` 作用于素材记录，所有序列中引用该 ID 的片段都会使用新文件。CLI 重新探测流信息、精确有理时长和 SHA-256 指纹，通过现有 `asset.put` 提交一次可撤销操作；工程协议版本、素材 ID、Stream ID、片段 ID、时间线/素材范围、特效、字幕与 LinkGroup 均不变。新文件更长也不会自动延长片段；更短导致任何已有范围或转场 handle 越界时，整次替换拒绝。

保留素材名称、标签、metadata、extensions 以及匹配流的 metadata/extensions；探测产生的流 metadata 同名键优先。URI、storage、availability、fingerprint、codec、index、duration 和视频/音频参数来自新文件；旧 mimeType、language 或颜色声明不会被当作新文件的信息沿用。无可靠色彩标记时可显式使用 `--color-space rec709|srgb|linear-srgb`，这不是颜色转换。

默认原素材为 managed 时继续托管，其余情况使用 external；`--managed` 强制复制，`--external` 强制引用原文件，两者互斥。托管替换生成独立文件，不覆盖或删除旧素材，撤销/重做仍可使用两份文件；外部文件需由用户保留。`--dry-run` 探测并验证拟议操作，不复制素材、不写快照或历史，返回的托管 URI 仅是候选路径。结果 JSON 包含 `assetId`、`usageCount`、`event`、`project` 和 `dryRun`。

普通媒体、静态图片和 `.cube` LUT 可替换，要求素材 kind 以及各类流数量一致。图片序列、字体和其他素材类型暂不支持此快捷入口，仍可通过完整领域操作编辑。只有一条同类流时自动按类型匹配，因此新文件中的音视频索引可以交换；多音轨等存在多条同类流时必须明确映射全部流：

```sh
cutcraft -p demo.cut asset info ASSET_ID
cutcraft probe /path/to/new-multitrack.mkv
cutcraft -p demo.cut asset replace ASSET_ID /path/to/new-multitrack.mkv \
  --stream-map stream_v0=stream_v0 \
  --stream-map stream_a1=stream_a2 \
  --stream-map stream_a2=stream_a1
```

左侧是原素材的实际 Stream ID；右侧按 `probe` 的 codec_type 和 index 表示（video → `stream_vINDEX`、audio → `stream_aINDEX`、图片/封面 → `stream_iINDEX`）。映射必须完整、一对一且类型一致，不能靠数组顺序猜音轨。原 Stream ID 保留，实际解码使用新 index。需要改变流拓扑或只换某个片段时，使用独立导入及显式 `item.put`/batch。

探测、哈希和复制在项目编辑锁外进行，提交时重读最新快照；目标素材或项目身份被并发替换则拒绝，其他并发编辑保留并重新校验引用。直接引用素材的锁定轨道需要显式 `--override-locks`。后台已打开项目同样通过 ApplicationHost 执行，任务期间不能关闭项目，可取消。托管文件先验证后发布，失败不留下部分工程编辑；已有素材和历史保持可恢复。

## 从素材定位使用位置

```sh
cutcraft -p demo.cut asset usages ASSET_ID
cutcraft -p demo.cut item list --asset ASSET_ID
```

查询覆盖所有序列，识别媒体片段的素材引用、字幕和内置文字生成器的字体引用、内置 LUT 特效引用。每个片段只返回一行，`references` 列出引用角色、字段位置、流 ID 或特效 ID。同一片段两次使用同一个 LUT，`usageCount` 仍为 1；两次引用都列在 `references` 中。

使用计数包括禁用片段、禁用特效和未被主序列引用的序列，表示项目中的存储引用，不表示最终画面出现次数。`--unused` 表示没有上述已知引用；插件的私有参数和不透明扩展不会被猜测为引用，也不会据此自动删除磁盘文件。

每条定位结果包含：

- `sequenceId`、`sequenceName`、`isRootSequence`
- `trackId`、`trackName`、`trackKind`
- `itemId`、类型、名称、素材/流 ID、引用明细
- `timelineRange`、`sourceRange`、`timeReference: "sequence"`
- `itemEnabled`、`trackEnabled`（存储开关，不代表综合静音/solo/父序列之后的可见性）

## 查找片段、轨道和序列

```sh
cutcraft -p demo.cut item search '喜字'
cutcraft -p demo.cut item list --sequence SEQUENCE_ID --track TRACK_ID
cutcraft -p demo.cut item list --sequence SEQUENCE_ID --at 3/2
cutcraft -p demo.cut item info ITEM_ID
cutcraft -p demo.cut sequence list
cutcraft -p demo.cut sequence list '片头'
cutcraft -p demo.cut track list --sequence SEQUENCE_ID
cutcraft -p demo.cut track list '字幕'
```

`item search` 是 `item list` 的别名。关键字匹配片段 ID、名称、标签、字幕/内置文字内容，以及它直接引用的素材 ID、名称和路径；嵌套序列片段还匹配被引用序列的名称。`--asset`、`--sequence`、`--track` 使用完整 ID，多个条件取交集。`item info` 提供完整片段对象及所属序列、轨道，便于后续编辑。

查询默认覆盖所有序列；编辑命令的默认目标仍是 root sequence。编辑查询得到的非主序列片段时，传入返回的 `--sequence SEQUENCE_ID`。

`--at` 必须显式指定 `--sequence`，时间按有理数精确计算，匹配 `[start, end)`：包括开始时刻，不包括结束时刻。该筛选查找存储的片段范围，不计算转场扩展区间、可见性或源时间映射；这些渲染语义使用 `sample --sequence SEQUENCE_ID --at TIME` 查看。

## 沿嵌套序列向上定位

```sh
cutcraft -p demo.cut sequence usages CHILD_SEQUENCE_ID
```

返回直接嵌入该序列的父片段及其位置；继续对返回的父 `sequenceId` 查询即可逐层向上查找。不会把子序列的时间直接相加成主时间线时间：父片段可能裁剪、变速、循环、倒放或多次引用，无法用简单加法得到准确位置。所有 `timelineRange` 都属于返回行的 `sequenceId`。

输出顺序稳定：素材/序列按 ID，轨道和片段按项目存储顺序。当前返回全部匹配，无分页、内容识别或语义搜索。

## 图片序列

素材搜索新增 `--source-type file|image-sequence`。图片序列的成员 URI/解析路径也参与 `asset search --path`、关键字搜索及 `item search`；返回首个命中的帧索引和匹配数量，一组图片仍只占一个素材记录。查询使用位置仍定位整个素材的引用，不能据此断言某个片段采到了特定帧。

`asset info ASSET_ID --summary` 省略完整帧清单；`asset frames ASSET_ID --offset 0 --limit 100` 单独提供零基帧分页（limit 1–1000），返回实际文件路径、精确源时间范围和指纹。`sample` 的 `imageSequenceFrames` 可确认指定输出时刻使用的源帧。其他搜索列表仍返回全部匹配。详见 [图片序列](image-sequences.md)。
