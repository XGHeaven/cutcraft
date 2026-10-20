# Cutcraft 项目约定

适用于整个仓库。修改协议、CLI、核心库和渲染器时遵守以下约定；用户在当前任务中的明确要求优先。

## 项目定位与依据

- Cutcraft 是以 JSON 工程为核心的 Rust 剪辑 CLI。CLI 和未来客户端应复用同一套领域语义，协议不得绑定某个 UI、脚本语言或 FFmpeg 命令行。
- 新项目采用首个公开 Spec **0.1.0**，入口是 [规范总览](spec/README.md) 与 `spec/schemas/`。基础领域、效果、动画/转场、时间重映射及图片序列分别定义于同目录的 SPEC、DOMAIN_MODEL、EFFECTS、ANIMATION、RETIME 和 IMAGE_SEQUENCES 文档。
- 未发布内部 0.3/0.4/0.5 资源位于 [archive](spec/archive/README.md)，仅用于兼容与回归；不能按版本号大小判断其功能与公开 0.1.0 的关系。新增协议能力修改现行规范，不重新解释归档 Schema。
- [实现状态](docs/implementation.md) 说明实际支持边界，`src/schema.rs` 注册离线 Schema。旧文档可能保留 `working/` 等历史布局；当前新工程使用 `cache/`。发现规范、Schema、实现不一致时明确指出并修正，不通过放宽校验掩盖问题。

## 协议演进与兼容性

- 修改持久化字段、默认值、命令或求值语义前，先定义 JSON 表示、必填/可选规则、时间单位、边界条件、错误行为及版本兼容影响，再同步规范、Schema、实现和示例。
- **CLI 版本与协议版本独立管理。** CLI 版本在 `Cargo.toml` / `Cargo.lock`；新建工程协议版本在 `src/lib.rs`。修复渲染 bug、增加快捷命令，不自动意味着升级工程协议。
- 破坏兼容性的变化必须使用新协议版本和独立 Schema；不得原地重新解释旧版本数据。共享 Schema（例如生成器参数）也要检查其对旧版本的影响。
- 内部 0.3/0.4/0.5 快照和历史继续按各自版本读取、校验、编辑。新能力可以要求新版本，但要明确报错；不得静默升级、降级、丢字段或重建用户工程。
- 目前没有原地升级命令。不能仅修改 `specVersion` / `$schema` 冒充迁移。以后实现迁移时，需定义原子提交、校验、历史哈希保留、ID 保留及撤销边界，并提供旧版本回归用例。
- Schema 必须支持离线加载，新增 `$id` / `$ref` 要同步嵌入注册。现有 `cutstate.dev` URI 是协议标识，不因产品改名而批量替换。
- 未知核心字段应拒绝；扩展只能进入明确允许的 `extensions`、`metadata` 或插件参数，并按契约保留。未知插件数据可保存，不代表渲染器可以执行或忽略它。

### 版本与能力的一致性检查

- 协议升级、重置或新增能力时，必须核对完整使用链路：新建工程的默认版本和 `$schema` → 离线 Schema 注册与分派 → 领域校验 → CLI 导入/编辑入口 → 采样/渲染后端 → Operation/Event 与撤销。不能只更新版本常量或 Schema 就认为完成。
- 能力支持与协议版本的关系应集中声明并复用。新增功能入口不得散落硬编码的单版本判断（例如图片序列仅允许 `specVersion == "0.5.0"`）；应查询明确的能力支持集合。兼容版本和当前版本可能同时支持同一能力，不能用 CLI 版本、版本数值大小或“不是旧版”推断支持情况。仅当旧版本确有不同语义时，保留明确的版本分派。
- 修改前后都用 `rg` 搜索 `SPEC_VERSION`、`specVersion`、`eventVersion`、Schema URI、旧版本号和旧规范路径，逐项确认命中属于现行逻辑还是历史兼容。重点检查 `src/project.rs`、`src/schema.rs`、功能入口、渲染能力声明和测试；禁止不区分归档、组件版本与依赖版本的全局替换。
- `spec/README.md`、根目录规范正文及 `spec/schemas/` 必须共同描述当前完整协议。新增专题须从现行入口链接，不能让入口仍描述旧版、将已实现能力标为“后续草案”，或要求读者拼接多个历史草案才能理解当前版本。历史文本只放在明确标记的 `spec/archive/`，移动文件后检查引用与链接。
- 同步核对 CLI 的 `schema` / `capabilities` 输出、示例、README、实现状态、相关使用指南、技能和 AGENTS；CLI 版本有变化时再核对 Cargo.toml / Cargo.lock、Changelog、安装与发布示例。协议版本、应用版本和效果/动画等组件版本分别处理。
- 回归必须验证当前版本的新工程能够实际执行相关功能，不能仅验证版本字符串或 Schema 通过；同时覆盖仍受支持的旧版本、不支持版本的明确拒绝、跨版本 Operation 拒绝，以及编辑/撤销后的工程版本、ID 和历史哈希保留。复用 [版本回归](tests/spec_versions.rs) 及对应功能测试，例如 [图片序列测试](tests/image_sequence.rs)。版本整理不得造成既有能力丢失，也不得自动改写用户工程。

## 工程、身份与精确时间

- `cutcraft.json` 是 canonical 工程快照，不使用 `project.json`。保留 `createdWith`、`lastModifiedWith`、`editors` 等应用版本记录；只读、dry-run 和拒绝操作不更新编辑者记录。
- 持久化实体使用带类型前缀的 UUIDv7，已有 ID 不随名称、路径、数组顺序变化；Stream ID 在 Asset 内定位。引用、查询和编辑使用实际 ID，不依赖显示名称唯一。
- 持久化编辑时间统一为约分后的 `{ "num": ..., "den": ... }` 有理秒，分母为正、零为 `0/1`，序列化遵守 i64 范围。计算复用 `src/time.rs`，不以浮点秒往返替代精确运算；颜色、几何参数等非时间数值不受此限制。
- 时间区间遵守左闭右开；倒放和上界采样使用协议的边界方向/`exclusive-upper`，禁止靠浮点 epsilon 修补边界。
- 变速以时间线时间到素材时间的映射表达；速度倍率是编辑输入或派生量，不另存一套可与映射冲突的状态。裁剪、切分、倒放、定格、非匀速和嵌套需保持映射语义。
- 属性动画复用通用曲线和注册槽位，时钟字段使用 `timeReference`。新增可动画属性需定义值类型、所属实体时钟、插值方式和验证规则，不另造互不兼容的关键帧格式。
- FFmpeg PTS/DTS、`time_base`、解码索引和临时路径属于后端或缓存，不写入 canonical 编辑时间模型。

## 编辑事务、锁与历史

- 生产路径通过领域 Command / Operation 修改工程，复用 `edit::apply` 和 `Store`；常用能力应提供 CLI 入口，不要求用户编写 Python 或直接改 JSON 才能完成。
- `put` 完整替换实体；保留契约要求的扩展数据。涉及多个实体的一次编辑用 batch 原子提交，并维护引用、LinkGroup、转场 handle、轨道约束和嵌套无环等不变量。
- 成功 Operation（含 batch）只增加一次 revision，并生成可审计 Event 和 inverseCommands；检查 baseRevision、operationId 和锁定轨道。失败不能留下部分编辑；符合契约的语义拒绝可以记录 rejected event。
- `--dry-run` 不写快照或历史。Undo 是新的领域操作，不删除历史或重写旧事件的哈希。
- 所有协作式工程读写走 `Store`，由 `.cutcraft.lock` 的 **OS 排他锁**互斥。PID 仅供诊断；不能以 PID 检查替代锁，也不能删除/替换锁文件来“解锁”。
- 快照和历史通过 `.cutcraft/transaction.json` 耐久事务日志恢复，并原子替换文件；新增写路径不得绕过恢复、同步落盘与锁释放机制。
- 渲染捕获已验证的 revision 后释放编辑锁；输出说明对应 revision。失败不覆盖已有成片，成功后原子发布，覆盖需遵循 `--overwrite`。

## 素材、渲染与可观测性

- 原始素材不作为可删除缓存。新项目使用 `assets/`、`cache/`、`output/`；`.cutcraft/` 是恢复数据，不是缓存。不要自动清理旧 `working/`，其中可能仍有素材引用。
- 图片序列导入时将 pattern、编号范围和缺帧策略解析为固定有序帧清单，保留指纹和精确帧率；之后采样不能重新扫描目录推断成员或顺序。
- 区分“协议允许”“CLI 可编辑”“后端可渲染”。不支持的启用效果、采样方式、颜色空间或布局明确报错，不静默跳过、替换或生成看似成功的结果。
- 颜色处理遵守声明的空间和 straight/premultiplied alpha 约定；透明度不能仅通过变量名或滤镜字符串推断正确，必须验证实际像素和合成结果。
- 媒体进程复用 `src/media_process.rs` 的错误处理，区分缺失程序、权限错误和处理失败。使用参数数组，不拼接 shell；文字通过文本文件传入并关闭表达式展开。
- stdout 保持最终结果 JSON；进度走 stderr，`--progress json` 使用 JSONL。进度应区分合成、解码、混音、编码等阶段，未知总量/ETA 不虚构，只有成功落盘才报告任务完成。参见 [文字与进度](docs/text-and-progress.md)。
- 可重建的演示工程、素材和发布产物分别放在 Git 忽略的 `local-examples/`、`dist/` 等位置。用户指定工程位置时遵循该位置，不放进源码目录；不自动改动真实素材作为测试。

## 验证与交付

- 协议变更覆盖合法/非法数据、旧版本兼容、精确边界及与变更相关的原子性/撤销/恢复测试；新增字段不能只改 Schema 而缺失语义校验。
- 渲染变更验证实际导出的像素、帧数或音频样本。透明文字要检查文字画布内部、字形外部的底图，以及半透明字形、预览和嵌套；“两个输出相似”或“校验通过”不能单独证明正确。
- 媒体测试设置 `CUTCRAFT_REQUIRE_FFMPEG=1`，避免因缺少依赖而跳过后仍宣称通过。修改范围内的检查通过后，只有新变更或未解决问题才扩大、重复测试；纯文档修改检查内容和链接即可。

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
CUTCRAFT_REQUIRE_FFMPEG=1 cargo test --locked
```

- 命令或能力变化同步相关 `docs/`、README、Changelog 和 `skills/cutcraft/`，明确已实现能力与限制。输出复核如只做抽帧或信号检测，不能声称已完成完整主观试听。
- Rust MSRV 当前为 1.86，构建使用 Cargo.lock。CLI 通过 GitHub Releases 分发，不发布 crates.io（`publish = false`）；发布检查、版本 tag 和本地演练遵循 [发布指南](docs/releasing.md)。
- 发布仓库为 `XGHeaven/cutcraft`，安装脚本默认从该仓库获取最新正式 Release，并支持 `--repo` 覆盖。修改分发仓库时同步安装脚本、下载示例、技能及安装回归测试；设置默认地址不代表已经发布。
- 安装脚本本身从仓库 `main` 分支的 `scripts/install.sh` 获取，独立于 CLI 发版；二进制和 SHA256SUMS 从同一固定 Release tag 下载。不要将脚本入口重新绑定到 Release 附件；调整脚本分发方式时同步发布打包及安装回归测试。
- 分发矩阵覆盖 Linux x86_64 / ARM64 / ARMv7 GNU、macOS ARM64 / x86_64 与 Windows x64 MSVC；以 `scripts/release.py` 的 TARGETS 为目标集合，同步 CI、安装器和文档。Unix 用 tar.gz，Windows 用 ZIP 和 main 分支的 `scripts/install.ps1`，安装入口不随二进制打包。
- 交叉打包必须提供 runner 并运行实际目标二进制，不能跳过执行验证；Linux ARM 使用 QEMU 库测试及工程创建/校验，不宣称经过原生 ARM 媒体验证。Windows 执行完整媒体测试及 PowerShell 5.1/7 安装回归。新增平台上线前以 CI 的实际通过结果为准。
- 文件持久化复用 Store 的平台同步函数；Windows 不支持通过 std 对目录 fsync，文档须说明掉电持久性差异，文件内容仍需用可写句柄同步。不能把 Unix 目录打开/同步写法散落到功能入口。
- 更新协议版本、工程布局或发布策略时，同时更新本文件中的对应约定，避免后续开发继续使用过时入口。
