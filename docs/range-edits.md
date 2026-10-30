# 按时间区间切除

`sequence cut` 在同一序列时间区间内裁开片段、移除选中内容，并按需收拢后续内容。全部轨道和部分轨道都支持，使用真实轨道 ID；选择范围必须明确给出。

```sh
# 全部轨道移除 [2, 5)，后续内容前移 3 秒（默认 remove-gap）
cutcraft -p demo.cut sequence cut --start 2 --duration 3 --all-tracks

# 只处理选定轨道，可重复 --track
cutcraft -p demo.cut sequence cut --start 2 --duration 3 \
  --track VIDEO_TRACK_ID --track AUDIO_TRACK_ID

# 删除内容、保留中间空隙
cutcraft -p demo.cut sequence cut --start 2 --duration 3 \
  --all-tracks --ripple none

# 先查看完整结果和影响清单，再执行；非主序列传 --sequence
cutcraft -p demo.cut sequence cut --sequence SEQUENCE_ID \
  --start 1001/30000 --duration 1/2 --all-tracks --dry-run
cutcraft -p demo.cut history undo
cutcraft -p demo.cut history redo
```

## 时间与作用范围

- 时间为所属序列的精确有理秒，支持整数、小数、分数；起点非负、时长必须大于零。区间左闭右开，恰好在区间结束处开始的片段保留，波纹模式下前移整个删除时长。
- 区间终点不能超过编辑范围：序列时长、所有存储片段结束点和标记结束点的最大值。禁用轨道/片段也算存储内容，solo、mute 和 enabled 不缩小选择范围。
- `--all-tracks` 与一个或多个 `--track ID` 互斥；未知、重复或其他序列的轨道 ID 明确拒绝。枚举了全部轨道 ID 等同全轨道选择。未选轨道不会被隐式扩选。
- `--ripple remove-gap` 为默认：所有选中轨道的后续内容前移同样的时长，包括原本就有空隙的轨道；未选轨道不移动。`--ripple none` 保持幸存内容原时间位置。
- 跨区间两端的片段保留前后两部分；只跨一端则裁短；完全覆盖则移除。第一段幸存部分保留原 Item/Effect ID，第二段分配新 ID；未受影响或只移动的片段保留完整数据。素材文件和素材记录不变。

## 关联、转场与标记

关联组中只要有成员会被裁切、删除或移动，所有成员所属轨道都必须被选中，否则报 `LINK_SCOPE_CONFLICT`。可补选轨道，或先显式 `link.remove` 解组。这里使用相同的**绝对序列区间**，不按各片段自己的相对偏移分别切。受影响的 LinkGroup 按区间前后幸存部分重建，各侧至少两个成员才成组；单个成员解除关联。首个幸存组沿用原 Link ID，另一组使用新 ID；metadata/extensions 保留。前后两部分因此可独立移动。

切除区间与转场窗口相交、转场任一端被删除、或连接边缘被切掉时，移除该转场，并在结果中列出 ID。区间外转场保留，引用改到相应的幸存片段；不会在新接缝自动添加转场。如果保留下来的转场缺少时长或 source handle，整次操作拒绝，不默默缩短转场。

`--markers auto|cut|keep` 控制序列标记：

| 策略 | 行为 |
| --- | --- |
| `auto`（默认） | 选中全部轨道时裁切标记；部分轨道时保持标记 |
| `keep` | 标记始终不动，也不删除 |
| `cut` | 对序列标记使用同一切除区间和 ripple 模式；即使只选部分轨道也生效 |

区间内的点标记删除，位于结束点的点标记保留；波纹模式下后续标记前移。范围标记按交集裁短；跨越整个区间时，保留空隙模式拆成两个标记（第一段保留 ID），波纹模式合为一个缩短标记。完全落在区间内的范围标记删除；自定义数据保留。

## 变速、动画、字幕与嵌套

复用现有精确切片：fit、倒放、定格、非匀速 timeMap、注册属性动画和特效动画都保留幸存部分的原采样结果，局部曲线重定位到新片段起点。字幕 word ranges 按交集裁切并重定位，字幕 text 保留原文；不会猜测删字或重新生成旁白。

以下表示暂不能保证无损的**部分裁切**，整次操作返回 `UNREPRESENTABLE_EDIT`：循环素材、非零 `fadeIn`/`fadeOut`、旧式 effect automation。完整删除和整体移动它们不需要切片，仍支持。可先使用明确的替代片段/曲线处理这些情况；命令不会静默丢弃自动化或淡入淡出。

父序列里的嵌套片段按自己的 sourceRange/timeMap 裁切，子序列内容不变。直接编辑子序列后，如果缩短的时长让父片段 sourceRange 越界，整个操作拒绝；不会擅自裁短父序列。

## 序列时长、锁与历史

自动时长由剩余启用内容和标记计算，允许空序列；即使 `--ripple none`，删除末尾内容也可能缩短自动时长。如需保留空白尾部，使用固定时长序列。

固定时长仅在**全轨道 + remove-gap**时减去切除区间与原播放范围的交集长度；部分轨道或保留空隙模式不改固定时长。减至零时改为 auto 并移除 duration（协议不允许 fixed=0）。原本位于固定播放范围之外的存储内容同样参与选中区间编辑，仍可能留下内容；完全空序列可继续编辑，但不能导出零时长视频。

修改到锁定轨道或其关联内容会拒绝，除非显式传 `--override-locks`。整次编辑由共用库生成完整 `sequence.put`，通过 ApplicationHost/Store 原子执行，只增加一次 revision 和 Event；失败不留下部分片段变化。`--dry-run` 不写快照和历史。成功结果含 `cut` 影响清单：轨道、区间、删除/裁短/拆分/移动的 Item ID、关联组变化、移除的转场、标记变化、前后时长与时长模式，以及完整拟议/已提交工程。

Event metadata 记录 `cutcraft.rangeEdit` 说明，Event command 已包含实际新 ID，因此回放不重新随机生成 ID。撤销/重做恢复完整序列并保留既有历史哈希。沿用现有 Operation/Schema，公开 Spec 0.1.0 和内部 0.3/0.4/0.5 均可使用其能够表达的切片；没有新增协议字段或自动升级工程。
