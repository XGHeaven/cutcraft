# Cutcraft Spec 0.5.0

> 未发布内部版本归档。现行公开协议见 [Spec 0.1.0](../../README.md)。

CLI 0.5.0 新项目使用本目录的独立 Schema。继承 0.4 的动画、转场和时间映射，新增 [图片序列来源](IMAGE_SEQUENCES.md)。0.3/0.4 快照及历史仍按各自 Schema 校验，不隐式迁移。

- [使用指南](../../../docs/image-sequences.md)：实际命令和实现限制。
- [完整示例](examples/image-sequence.cutcraft.json)：三帧占位素材及一个时间线片段，仅用于离线协议校验，不附源图。
- `schemas/`：完整 project/common/operation/event/animation/effect 资源，URI 使用 v0.5。

```sh
cargo run --locked -- validate spec/archive/0.5.0/examples/image-sequence.cutcraft.json
CUTCRAFT_REQUIRE_FFMPEG=1 cargo test --locked --test image_sequence
```

图片序列只在 0.5 项目内可用。没有原地协议升级命令，不应手动修改旧项目版本字段及历史。
