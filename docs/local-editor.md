# 本机网页编辑器

`open` / `close` 管理项目会话。工程仍使用 Spec 0.1.0 和 `cutcraft.json`；网页与 CLI 共用 Rust 应用服务、领域 Operation 和 Store。

```sh
cutcraft init my-film.cut
cutcraft open my-film.cut
cutcraft -p my-film.cut import /absolute/path/video.mp4 --append --color-space rec709
cutcraft -p my-film.cut history undo
cutcraft -p my-film.cut history redo
cutcraft close my-film.cut
```

`open --no-browser` 只注册项目并返回连接地址。省略位置时使用 `-p` 或当前目录。第一个项目启动用户级后台进程，其他项目共享同一个监听端口（默认 loopback）；重复打开真实路径相同的工程是幂等的。关闭网页标签不关闭项目。`close` 作用于项目的所有网页会话；最后一个项目关闭后停止监听并退出。导入或导出进行中返回 `PROJECT_BUSY`，须完成或取消任务后再次关闭。

只有已打开项目的 CLI 请求转发给后台进程；未打开项目使用进程内 ApplicationHost，执行后退出。工具命令如 `id`、`schema` 不需要服务。进度仍写 stderr，最终结果仍为 stdout JSON。

## 编辑与历史

主界面保留素材库、监视器和时间线。项目名称打开项目管理，Import、Export、Tasks 各自打开弹窗；选中片段后点击 Properties 或双击片段打开详细属性。弹窗支持 Escape 关闭，并隔离时间线快捷键。

编辑器针对 PC 浏览器布局，默认使用 14px 基础字号与 36px 操作按钮；预览和时间线随窗口宽高分配空间。工具栏空间不足时换行，精确跳转与缩放入口保持可用，时间线在自身区域内横向滚动。

网页提交带有其看到的 `baseRevision`。CLI 和网页同时编辑时，陈旧请求明确冲突，刷新后再操作；服务不会把旧的完整实体覆盖到新 revision。拖动中以临时 Command 生成草稿静帧，结束时才提交一次 Operation，草稿不写快照或历史。后台预览和导出使用固定快照，不持有工程编辑锁。

`history undo` / `history redo` 操作整个项目共享的多步历史，包含 CLI 与网页的改动。新编辑清空 redo，拒绝操作和 dry-run 不入栈。历史栈可由 `history.jsonl` 中的 Event 重建；撤销与重做生成新事件，不删除或改写旧事件。原来的 `undo` 保留“撤销最新 applied Event”的兼容行为，旧 undo 事件作为普通提交参与新历史栈。

新历史操作在允许的 Event `metadata["cutcraft.history"]` 中保存 `{ "version": 1, "action": "undo" | "redo", "targetEventId": "..." }`。只有目标、命令和栈顺序相符的标记被识别。此为应用约定，不改变领域命令或旧工程协议。redo 应用对应 undo Event 的逆命令，保留身份和引用。

工程中的颜色等小数采用精确的浮点序列化往返，保证提交前后哈希一致。已经不连续的历史或被直接修改过的快照会返回 `UNDO_CONFLICT`，不会改写旧事件的哈希。

## 预览

浏览器展示 Rust 合成的静帧与音视频，不另行实现效果或时间映射。暂停、逐帧和拖动位置使用 PNG；连续播放使用 H.264/AAC fragmented MP4 和 Media Source Extensions，需要支持相应编解码器的桌面浏览器。网页默认 0.5× 分辨率，可在监视器选择 1× / 0.5× / 0.25×，相对于序列原始尺寸；播放最高 24fps。切换比例会暂停播放并保留位置，重新生成对应清晰度的静帧。首次解码、复杂效果和慢设备可以缓冲，不保证实时吞吐。视频预览按实际帧时间戳选取包含目标帧及相邻帧的有界解码窗口，跳过前置解码帧后再应用缓存容量限制；图片序列缩略图使用导入时保存的帧清单。

编辑提交后旧播放暂停，网页更新到新 revision；重新播放会创建新的播放会话。解码保留素材的真实 PTS 和源时间边界，工程时间仍使用精确有理秒。预览和媒体缓存可重建，不属于原始素材或恢复数据。

交互预览能力按明确版本集合声明：公开 0.1.0 与内部 0.4/0.5。内部 0.3 保留原有 CLI MP4 预览、编辑和导出，不隐式升级。未知效果或不支持的渲染能力明确报错。

### 连续分辨率参数

`GET /api/v1/projects/{handle}/preview/frame` 的 query，草稿 `POST .../preview/frame` 与播放 `POST .../preview/playback` 的 JSON 均接受可选 `scale` 数值。例如 `scale=0.37`，或 `{ "scale": 0.25, ... }`。要求有限实数且 `0 < scale <= 1`，无效值明确拒绝。省略时兼容原有最长边 640 的预览；不带选项的 CLI `preview` 保留原有整段 MP4 行为；单帧/区间选项复用此引擎。

比例应用于解码、嵌套画布、文字尺寸与合成，而非只在编码末尾缩小。宽高分别向下取偶数，最小 2；输出每边不超过 4096、按 16×16 编码块补齐后的总像素不超过 9437184，超限明确要求减小比例。元数据返回实际 `width` / `height` / `scale`，H.264 level 随实际尺寸选择。缓存键包含比例，静帧、草稿和播放流使用同一规则，比例不改变工程或导出格式。

普通正向、1:1 时间映射、固定音量/声像且无淡入淡出的音频按块混合，使用精确时间锚点计算索引和分数相位；其他映射保留逐采样求值路径。视频在恒等几何且无效果的 Rec.709 输入上跳过无效变换，16 位输入的传递函数使用逐值精确查表。可在 debug 构建下复测阶段耗时：

```sh
CUTCRAFT_PROFILE_PROJECT=/absolute/path/project.cut CUTCRAFT_PROFILE_SCALE=0.5 cargo test --locked --lib profile_preview -- --ignored --nocapture
```

## 本机 API 与运行状态

服务默认监听 `127.0.0.1` 的动态端口。`open --host localhost`、loopback IPv4/IPv6 地址免登录；`open --host 0.0.0.0`、`--host ::` 或其他 IP 地址启用一次性启动码和会话认证。`--host` 接受 IP 字面量或 `localhost`，不做任意主机名解析。首个项目确定监听地址，后续省略 `--host` 复用现有服务；显式指定不同地址返回 `HOST_BIND_CONFLICT`，须关闭全部项目后重新打开。通配地址返回本机可访问的 loopback URL；其他设备使用本机对应的 IP 和该端口。用户运行目录默认为 Unix 的 `~/.cutcraft/run` 或 Windows 的 `%LOCALAPPDATA%/.cutcraft/run`；测试可设置 `CUTCRAFT_RUNTIME_DIR`。其中的 OS 锁决定服务是否存活，PID 仅作诊断，不能删除锁文件解锁。

`/api/v1` 与工程 `specVersion` 独立。CLI 保留私有 Bearer 凭证。仅绑定 loopback 时认证默认关闭，`open` 返回普通 `?project=...` 地址，可直接打开和刷新，不需要启动码或 cookie；旧地址中的启动码会被忽略并清除。绑定非 loopback 时（包括通配地址）认证对所有连接启用，即使请求来自本机也不能绕过；网页用 URL fragment 中的一次性启动码换取 HttpOnly 会话 cookie，并立即清除 fragment。两种模式均校验 Host/Origin；允许与绑定匹配的 IP 字面量，loopback 可用 `localhost`，Origin 必须匹配访问地址。媒体通过登记资源提供。不要分享远程认证模式的启动码或运行目录凭证。

项目变化和任务进度通过 SSE 通知，网页重新读取权威快照。断线后重新同步。请求在同一实例内按 requestId 去重；服务重启或结果过期后，未知结果不自动重执行，也不自动转为本地执行。先查询工程和历史再决定下一步，尤其注意 `import --append` 保留导入和追加两次提交的既有语义。

## 从源码构建网页

网页源代码在 `web/`，发布资源随二进制嵌入，运行时不需要 Node.js。修改网页后更新嵌入资源：

```sh
cd web
npm ci
npm run build
cd ..
cargo build --locked
```

校验包括 Rust 1.86、现有分发目标、强制 FFmpeg 媒体回归、前端类型检查和浏览器实际播放。跨平台 CI 通过前不能宣称新增路径已在所有平台验证。
