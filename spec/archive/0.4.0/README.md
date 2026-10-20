# 0.4.0 动画协议草案

> 未发布内部版本归档。现行公开协议见 [Spec 0.1.0](../../README.md)。

入口：[动画与转场设计](ANIMATION.md)、[变速与时间重映射](RETIME.md)。CLI 0.4.0 已实现核心能力，使用方式与后端边界见 [动画与变速指南](../../../docs/animation-and-retime.md)。上级目录保留 0.3.0 兼容规范。

本目录的 schemas 是独立完整的快照，URI 使用 v0.4。CLI 分版本加载完整资源集合，不能混用 v0.3 common 与 v0.4 project。

完整项目示例：

- [移动、旋转、淡入和曝光变化](examples/animated.cutcraft.json)
- [带贝塞尔进度的交叉淡化](examples/transition.cutcraft.json)
- [加速、定格与倒放](examples/retime.cutcraft.json)（引用占位素材，不附视频）

校验与参考求值测试：

```sh
python3 -m venv /tmp/cutcraft-spec-venv
/tmp/cutcraft-spec-venv/bin/pip install 'jsonschema>=4.22,<5'
/tmp/cutcraft-spec-venv/bin/python spec/archive/0.4.0/validate.py
```

校验器检查 Schema、新增动画时间/缓动约束及转场窗口，包含负例、插值求值与精确时间重映射测试；它不替代完整项目语义、媒体 handle 校验或渲染后端。未知插件必须由对应插件验证。当前 CLI 使用这里的 Schema；修改协议时应同步 Rust 实现、Schema 与示例。
