# Cutcraft Project Specification v0.3

> 未发布内部版本归档。现行公开协议见 [Spec 0.1.0](../../README.md)。

Cutcraft 是一个面向 CLI、可视化编辑器和 AI Agent 的 JSON 非线性剪辑工程规范。本目录包含规范文本、JSON Schema、示例工程和参考校验器。

## 文件

- `SPEC.md`：领域模型、时间模型、渲染语义和修改协议。
- `DOMAIN_MODEL.md`：聚合、实体、值对象、不变量和领域服务。
- `schemas/common.schema.json`：公共值对象定义。
- `EFFECTS.md`：变换、调色、RGB 曲线、LUT/.cube 的规范语义。
- `schemas/effect.schema.json`：内置效果参数 Schema。
- `examples/luts/`：1D/3D identity .cube 示例。
- `schemas/project.schema.json`：项目快照 Schema。
- `schemas/operation.schema.json`：原子领域命令 Schema。
- `schemas/event.schema.json`：历史事件 Schema。
- `examples/minimal.project.json`：最小合法项目。
- `examples/full.project.json`：包含视音频、字幕、转场、嵌套序列和效果的项目。
- `examples/batch.operation.json`：原子批处理命令。
- `examples/history.jsonl`：事件日志示例。
- `scripts/validate.py`：Schema 和语义约束参考校验器。

## 设计原则

1. **Data-first**：`cutcraft.json` 是唯一工程事实来源。
2. **稳定身份**：持久化实体使用带类型前缀的 UUIDv7，ID 创建后不可修改。
3. **统一精确时间**：所有持久化编辑时间都使用 `{num, den}` 有理秒；PTS、DTS 和媒体 `time_base` 仅属于后端或可重建索引。
4. **领域命令**：CLI 通过 `item.move`、`item.trim` 等命令修改项目，不直接暴露 JSON Patch 路径。
5. **确定性**：相同项目、素材指纹、插件版本和渲染配置必须产生语义等价输出。
6. **可扩展但不含糊**：厂商扩展只能进入 `extensions`，自定义效果和生成器使用反向域名标识。

## 建议工程布局

```text
my-project.cut/
├── cutcraft.json
├── .cutcraft.lock # OS 排他锁及持锁者诊断信息
├── .cutcraft/     # 耐久事务日志，不是缓存
├── history.jsonl
├── assets/
├── working/       # 可删除、可重建；可包含 PTS/DTS、关键帧和媒体索引
└── output/
```

## 校验

参考校验器依赖 Python 3.11+ 和 `jsonschema`：

```bash
python -m pip install 'jsonschema>=4.22'
python scripts/validate.py
```

## 状态

这是可实现的 `0.3.0` 草案。版本 `0.x` 期间允许不兼容修改；任何实现都必须读取 `specVersion`，不得静默猜测版本。

0.3.0 新增项目应用版本记录、跨进程锁契约、[视觉效果规范](EFFECTS.md) 与 `schemas/effect.schema.json`。

## 下一版设计

[0.4.0 通用属性动画与转场草案](../0.4.0/ANIMATION.md) 包含独立 Schema、完整项目示例和参考校验/求值器。CLI 0.4.0 已嵌入这套 Schema 并实现核心能力，上面的 0.3.0 文件用于兼容。

0.4.0 另含 [变速设计](../0.4.0/RETIME.md)：fit 匀速、timeMap 非匀速、精确 Hermite 速度渐变、独立画面采样与音频政策。后端已实现精确 timeMap、帧混合及显式音频重采样，具体限制见 [使用指南](../../../docs/animation-and-retime.md)。

[0.5.0 图片序列规范](../0.5.0/IMAGE_SEQUENCES.md) 定义一个 Asset 对应有序图片清单、视频流帧率、导入与缺帧策略、指纹和搜索/采样行为。CLI 0.5.0 已实现，新项目使用 [0.5 Schema](../0.5.0/README.md)，旧项目不隐式迁移。
