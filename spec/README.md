# Cutcraft Project Specification 0.1.0

这是首个公开版本的协议，包含当前全部已定义能力：JSON 工程、精确时间、原子编辑与历史、视觉效果、动画、转场、时间重映射和图片序列。CLI 与 Spec 本次均从 `0.1.0` 起步，之后独立管理版本。

## 规范与 Schema

- [SPEC.md](SPEC.md)：领域模型、时间、渲染、命令、版本与文件契约。
- [DOMAIN_MODEL.md](DOMAIN_MODEL.md)：聚合、实体、不变量与领域服务。
- [EFFECTS.md](EFFECTS.md)：几何变换、调色、RGB 曲线、LUT 及可动画高斯模糊。
- [ANIMATION.md](ANIMATION.md)：通用属性动画、注册槽位与转场。
- [RETIME.md](RETIME.md)：精确时间映射、非匀速、倒放和采样政策。
- [IMAGE_SEQUENCES.md](IMAGE_SEQUENCES.md)：图片序列来源与固定帧清单。
- [schemas/](schemas/)：完整的 project/common/operation/event/animation/effect Schema，标识使用 `https://cutstate.dev/schema/v0.1/`，离线加载。

上述专题均是本版本的规范性组成部分。效果、动画、生成器、图片序列来源及处理器自己的 `version` 独立于项目协议；现有 `1.0.0` / `2.0.0` 语义保持不变。实际 CLI 与渲染支持边界见 [实现状态](../docs/implementation.md)。

## 工程布局

```text
my-project.cut/
├── cutcraft.json
├── .cutcraft.lock # OS 排他锁及持锁者诊断信息
├── .cutcraft/     # 耐久事务日志
├── history.jsonl
├── assets/       # 原始或受管素材
├── cache/        # 可删除、可重建的缓存
└── output/
```

旧工程可能仍引用 `working/` 内的文件，不自动移动或删除。

## 示例与校验

- [最小工程](examples/minimal.cutcraft.json)
- [高斯模糊动画](examples/blur.cutcraft.json)
- [属性动画](examples/animated.cutcraft.json)
- [转场](examples/transition.cutcraft.json)
- [时间重映射](examples/retime.cutcraft.json)
- [图片序列](examples/image-sequence.cutcraft.json)
- [原子操作](examples/batch.operation.json)、[生成的事件](examples/history.jsonl) 与 [执行后快照](examples/after-operation.cutcraft.json)：应用于最小工程，历史哈希对应示例输入与输出。
- [1D/3D identity LUT](examples/luts/)

示例中的媒体引用可能是占位资源，适合 Schema/语义验证；媒体校验或渲染需真实素材。

```sh
cargo run --locked -- validate spec/examples/animated.cutcraft.json
cargo run --locked -- validate spec/examples/batch.operation.json --kind operation --level schema
cargo test --locked --test spec_versions
```

生产校验使用 Rust CLI，旧 Python 参考求值器仅留在归档中。

## 发布前版本归档

内部迭代编号 `0.3.0` / `0.4.0` / `0.5.0` 未公开发布，已归入 [archive/](archive/README.md)。公开 `0.1.0` 包含这些迭代完成的能力，不能用版本数值大小判断功能多少。读取器按明确支持的版本集合选择 Schema 和语义。

已有工程、历史、ID 和哈希不自动改写；内部版本仍可按原版本读取和编辑。没有原地迁移命令，不能仅替换版本字段。后续公开协议按正常版本顺序演进；未知版本必须明确拒绝。
