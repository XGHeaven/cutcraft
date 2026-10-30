# Implementation status and choices

Cutcraft 0.1.0 creates Spec 0.1.0 projects using `spec/schemas/`. This is the first public baseline and includes all capabilities from the unpublished 0.3/0.4/0.5 drafts in `spec/archive/`. Existing draft snapshots and history remain supported without implicit migration. See [animation and retime](animation-and-retime.md) and [image sequences](image-sequences.md). Image-sequence import supports public 0.1.0 and internal 0.5.0 projects; 0.3/0.4 require a separate new project. There is no in-place migration command. The canonical snapshot is `cutcraft.json`; legacy 0.2.1 projects are rejected without automatic migration. This document distinguishes the implemented contract from rendering limits.

| Area | Implemented | Boundary |
| --- | --- | --- |
| Project/Asset/Stream/Sequence/Track | Schema, references, CRUD, ordering | Local media resolver only |
| All five Item types | Storage, validation, edits, timeline sampling | Renderer supports media, nested sequences, generators, captions and adjustment layers |
| Time | Exact BigRational arithmetic and canonical reduction | Checked signed 64-bit JSON integers; larger values rejected |
| IDs | Prefixed lowercase UUIDv7 | Stream IDs remain asset-local |
| Operations | All 19 command variants, atomic batch, revision guard | Nested batch is rejected by upstream Schema |
| Links | Bidirectional membership, group edits | Linked overwrite targets must be unlinked explicitly |
| History | Applied/rejected Events, inverse commands, hashes | No branch history or arbitrary historical undo |
| Persistence | Advisory process lock, fsynced redo journal, atomic individual file replacement, recovery | Cooperating readers must use Store; external tools reading both files during a write can see different revisions |
| Undo | Legacy undo plus shared multi-step history undo/redo; all append new Operations | Inverse restore can require `--override-locks`; missing top-level optional metadata/extensions may restore as equivalent empty objects |
| Schema validation | Offline embedded JSON Schema 2020-12 | Embedded schema URI retains original Cutstate namespace |
| Semantic validation | Global IDs, graph cycles, kinds, bounds, overlap, links, crop, words, automation, handles | Custom effect/generator semantics are plugin-defined |
| Media validation | Paths, fingerprint, stream kind/index, actual source bounds, built-in parameter Schemas, unavailable-plugin rejection | Remote sources and general plugin loading are unsupported; explicit system-font names and pinned fonts are checked for missing glyphs |
| Search and locate | Asset name/path/tag filters; media/LUT/font usage lookup; item/track/sequence discovery and parent-sequence references | Snapshot queries only; local sequence times; no opaque plugin-reference inference or content recognition. See [search and locate](search-and-locate.md). |
| Text cards | Optional title/caption card, rounded backgrounds, linear gradients, inside borders, shadows, glyph strokes, explicit multiline layout | Public 0.1.0 only; static card fields, clipped overflow, no automatic wrapping or HTML/SVG. See [text cards](text-cards.md). |
| Preview/export | Streaming MP4 preview/export with concurrent video/audio production and encoding; CLI PNG/interval MP4, thumbnail sheets and boundary frame pairs using the interactive engine; local editor PNG frames and fMP4/MSE playback with continuous resolution scale | CPU rendering; buffering is possible; interactive preview requires public 0.1.0 or internal 0.4/0.5 |
| Cache management | Shared disk cache manager, LRU budgets, cross-process leases, grouped decode windows, staging recovery, `cache status/clean` | 1 GiB project target; active entries and current asset references may exceed limits; unknown files and symlinks are preserved. See [cache management](cache.md). |
| Local editor | `open` / `close`, shared ApplicationHost, project-scoped CLI routing, SSE refresh, jobs, shared undo/redo | Loopback only; close refuses busy imports/exports; no cloud collaboration or GPU promise |
| Image sequences | Frozen manifests, printf/hash discovery, ordered file lists, atomic import/append, exact video sampling, frame-path search and paginated inspection | Local static images, max 1,000,000 slots; full import/media validation, on-demand decoding with a per-render 64 MiB cache; no migration/watch/VFR input manifests |

Text transparency and phased stderr progress are described in [text and progress](text-and-progress.md).

Distribution builds cover Linux x86_64/ARM64/ARMv7 GNU, macOS ARM64/x86_64 and Windows x64 MSVC.
ARM targets use cross compilation with QEMU library and packaged CLI checks; media regression is native on the other platforms.
See [installation](install.md) for platform requirements and the v0.1.0 cross-platform re-release.
Windows flushes file contents and atomically replaces files, but Rust std has no directory fsync equivalent;
the Unix directory-sync guarantee does not extend to Windows power-loss durability.

## Transaction and recovery

The public `edit::apply` is pure: it clones the snapshot, applies commands, normalizes canonical fields, validates the entire result, computes inverses, then returns the next snapshot and Event. Failure cannot mutate the caller's snapshot. Final validation happens after all batch members, allowing mutually dependent entities to be created together. Track locks are checked after **each** subcommand to prevent unlocking/relocking through a batch bypass.

The CLI holds a nonblocking exclusive OS advisory lock on `.cutcraft.lock` for snapshot access and editing. PID/acquiredAt/application are diagnostic metadata written only after acquiring the lock; the file is never replaced or deleted. Contention reports PROJECT_LOCKED. `Store` writes an fsynced journal containing the complete next snapshot and complete next history, replaces both public files using synced temporary files, then deletes the journal and syncs its directory. Opening the project recovers any remaining journal first. `.cutcraft/` must not be treated as disposable cache. This correctness-first implementation rewrites the history on commit and is O(history size); a future segmented WAL can improve large-history performance.

Rendering captures a validated revision and releases the editing lock before encoding. Its output describes that revision. Files are staged and published only after successful encoding, and existing output requires `--overwrite`. Source files can still be changed externally during rendering; managed assets and externally enforced immutability are preferred for reproducibility.

The original no-option `preview` command produces `preview.mp4` under project-root `cache/preview-<cli-version>-640-24-v1-<hash>-<sequence>/`. The reference renderer caps the long edge at 640 pixels and frame rate at 24 fps, retaining smaller dimensions/lower rates and rounding dimensions down to even numbers (minimum 2). Source buffers, effects and nested sequence canvases are reduced before composition; text sizes and margins are scaled with their owning canvas. H.264 defaults to libx264 ultrafast / CRF 28 and audio is capped at 24 kHz. Project format and final export settings are unchanged. Each invocation renders the complete selected sequence again; source decoding still scans complete inputs. The JSON result returns the MP4 path in `video` and `output`, with dimensions, frame rate and revision. This offline command has no `--open` flag. The separate `open` command serves the embedded browser editor, with modal project/import/export/properties workflows and 1× / 0.5× / 0.25× preview presets. Its API accepts any finite scale in (0, 1], with even dimensions and explicit output size limits. Interactive unity-rate static audio is mixed in blocks with exact rational anchors; complex mappings retain the sample evaluator. Identity Rec.709 geometry skips resampling, and input color transfer uses an exact 16-bit lookup table; see [local editor](local-editor.md).

`cache/` is reserved for rebuildable data. Use `cache status` and `cache clean [--dry-run]` to inspect or reclaim it safely; do not remove it while media work is running. The shared manager adopts known existing layouts, preserves unknown files/symlinks/current asset references, and uses OS entry leases to avoid deleting active work. Automatic eviction reads a fresh Store snapshot; it defers if the project is locked. Lock coordination files remain in `cache/.manager/`. See [cache management](cache.md) for budgets and soft-limit behavior. Initialization creates `assets/`, `cache/` and `output/`; it no longer creates `working/`. Existing `working/` directories are left untouched because they may contain referenced media or editing inputs. Reference render and MP4 preview stream composed RGBA16 video and float32 audio to FFmpeg over two loopback TCP inputs, with socket backpressure and independent producers. They no longer spool whole composed video/audio files. Source decode staging and the final MP4 staging remain temporary and are cleaned up after the render; video frames are still composed sequentially. Progress stages can interleave. See [streaming export](streaming-export.md).

`undo` selects the latest applied Event, skipping rejected ones, and verifies both revision and snapshot hash. It does not delete history. Repeating legacy `undo` toggles the most recent change (undo-of-undo). The new `history undo` / `history redo` commands and the web editor use a shared multi-step stack reconstructed from validated Event metadata. Event inverses restore changed aggregates in full, including cascades and ripple edits. Lock overrides are external CLI capabilities, never project flags.

## Editing rules

- `put` creates or fully replaces an entity with the same ID. A repeated put is idempotent in **editorial state**, but each accepted operation still creates an Event and increases revision.
- Entity removal fails final validation if references remain. `cascade: true` removes dependent media/sequence items and cleans transitions/links. Removing a track removes its contents; cascade additionally cleans references to those contents. Removing the root sequence requires assigning another root in the same batch.
- `link.put` assigns all item back-references; it refuses to steal items from another group. `link.remove` removes back-references. Move/trim/remove/split expand to all group members.
- Move applies the same time delta to all linked members. `toTrackId` relocates the explicitly selected item only, because its AV partners may belong to another track kind.
- Trim uses the explicit complete new timeline/source ranges for the selected item. Linked fit items inherit the same timeline boundary deltas and map them through their original source rate. Each resulting object must pass semantic validation.
- Split removes the original Item, creates two caller-specified new IDs, creates fresh effect IDs on copies, and repairs transition endpoints. Linked members split at the same **relative offset from their own start**; two replacement LinkGroups retain membership and relative offsets. Caption word ranges are intersected and rebased.
- Splitting looping media at arbitrary phases cannot always be represented faithfully by a single `sourceRange` without a loop-phase field. Inline property and time-map curves support faithful slicing. Slicing loop items, legacy automated effects, or nonzero fade fields still returns `UNREPRESENTABLE_EDIT`. Explicit replacement items/automation remain possible through a batch. This is a deliberate limitation, not silent data loss.

### Ripple policy

The draft names ripple modes without fully defining their scope. Cutcraft uses these deterministic rules:

| Operation/mode | Resolution |
| --- | --- |
| `none` | Change only selected/link-group items; reject resulting illegal overlaps |
| Move + `insert` | Shift destination-track items starting at/after the insertion time by moved duration |
| Trim + `insert` / `remove-gap` | Shift followers starting at/after the old end by the new-end minus old-end delta |
| Move + `remove-gap` | Close the old source-track interval; the requested destination time is interpreted in the resulting timeline |
| Remove + `remove-gap` | Close removed intervals on affected tracks |
| `overwrite` | Intersect competing destination-track ranges, retaining left/right source-mapped slices or deleting fully covered items |

Followers' LinkGroups move together, once per operation. These item-level ripple commands do not move sequence markers. Clips straddling an insertion boundary are not implicitly split; if the resulting state violates overlap policy the operation fails. Overwrite rejects linked or transition-connected competitors rather than leaving their synchronization or handles ambiguous. Item removal accepts only `none` and `remove-gap`.

## Legacy 0.3 FFmpeg profile

Run `cutcraft capabilities` for the machine-readable profile. The backend accepts local readable media with CFR video and grid-aligned boundaries, normal video layering, square output pixels, stereo/mono audio, generators, and captions. Output is H.264/AAC MP4 with even dimensions.

Native PTS/time_base are read transiently from FFprobe; they do not enter canonical snapshots or Events. Source trim PTS and sample boundaries are computed using exact integer rational arithmetic. Video source times include the Asset's shared presentation origin, and audio trims use the same origin; the backend rejects requests before an individual stream's start rather than incorrectly zeroing that stream's sync offset. General VFR handling, delayed-stream padding and non-grid boundaries need a more complete decoder/sampler backend.

The CFR guard checks average/nominal rates and inspects packet presentation timestamps in sorted presentation order against the exact frame interval (rather than decoding the source twice). Missing, duplicate or nonuniform PTS are rejected. This metadata scan runs once per stream per render and is not cached yet; media decoding still occurs during rendering. The domain format supports VFR inputs, but this backend rejects them.

The backend intentionally rejects transitions, unknown effects/automation, loop/hold, non-normal blends, audio stretching, pan/fades and unsupported caption style fields. Unsupported data stays in the project. The default system font is convenient for prototyping; `fontAssetId` pins a specific file for subtitles. Unknown plugin JSON is preserved but not executed.

FFmpeg receives argument arrays without a shell, and title/caption content is passed through temporary text files with expansion disabled. Previews capture a fixed project revision.

## Remaining backend work

1. Optimize the disk-indexed reference renderer, including compressed intermediates, incremental decoding and delayed-stream padding.
2. Additional blend modes, optical-flow and preserve-pitch processors.
3. A general versioned plugin loader extending the built-in parameter Schemas and media-level availability checks.
4. Incremental preview caching/live refresh and scalable event storage.

## Application metadata and visual effects

New projects and successful edits record createdWith, lastModifiedWith and a deduplicated editors list before computing history hashes. Reads/dry runs/rejections do not record editors. Color-grade/curves/LUT parameters are schema-validated; curve order/endpoints, video-track placement, static automation, and LUT asset references are semantically validated. Transform semantics and .cube parsing/interpolation are specified in spec/EFFECTS.md. LUT importing/parsing, color/transform rendering, adjustment layers and recursive sequence composition are implemented. Color math runs in Rust at f64 precision on straight RGB in linear-sRGB, with explicit sRGB/Rec.709 transfer conversion and linear-space LUT strength blending. The decoder/encoder bridge streams RGBA16 frames into lossless FFV1 intermediates. Geometry uses inverse affine mapping, pixel-center sampling and premultiplied bilinear interpolation. This CPU reference path favors correctness over throughput; temporary lossless intermediates consume disk and it is not a real-time GPU preview. Disabled effects are skipped; unavailable enabled plugins fail closed. Existing user projects are not rewritten.

## Local application host

CLI command adapters now delegate to the reusable application host. Open projects run in one per-user service, bound to loopback by default; unopened projects run in-process without a listener. Project registration is keyed by canonical directory and is independent of the project ID. Closing the final project shuts down the service; browser disconnection does not close a project. Import preparation releases the Store lock before probing/hashing/copying and acquires a fresh Store for commit. Source files and frozen manifests remain canonical inputs.

Network API v1 is independent of project Spec versions. GUI writes require baseRevision. Events and job progress are sent through SSE, followed by authoritative snapshot refresh. Same-instance request IDs prevent repeated execution; uncertain outcomes after restart are not silently retried. See [local editor](local-editor.md) for commands, authentication and source builds.

Local loopback editor sessions require no startup code or cookie. `open --host` selects an IP literal (or localhost); non-loopback bindings, including wildcard addresses, enable startup-code/session authentication for all callers. Host and same-origin checks remain active in both modes. Explicit binding changes require closing all projects first; omitted `--host` reuses the active service. IPv4 and IPv6 URLs are supported.

## Gaussian blur and CLI preview selections

Public 0.1.0 projects support `builtin.gaussian-blur` 1.0.0 with static or animated `sigma` (0–100 original-input pixels). Ordered effects run before geometry; blur filters linear premultiplied RGBA with a separable normalized Gaussian and clamped edges. Reduced previews scale each axis by its actual source/canvas size. Internal 0.3/0.4/0.5 projects reject the new effect without rewriting their version or archived schemas.

`preview --at` reuses interactive PNG generation and caching; `--start` / `--duration` uses PreviewEngine for only the selected interval with exact shared video/audio origin and bounded source decoding. Optional `--scale` shares browser resolution validation. All outputs capture a fixed revision; no project/history mutation. The no-option preview retains its legacy full-sequence behavior. See [CLI previews](preview.md) and [visual effects](visual-effects.md).

## System fonts

Titles and captions accept `--font-family` (stored as `fontFamily`), including `PingFang SC` and exact PostScript face names such as `PingFangSC-Semibold`. System fonts are discovered by Cutcraft on macOS, Windows and Linux; macOS downloadable AssetsV2 fonts are included. The selected TTC face is staged as face zero for FFmpeg without modifying the source file. Explicit fonts fail on missing families, invalid files or absent glyphs rather than silently falling back. Existing `fontAssetId` retains precedence and face-zero behavior. New fonts require a new CLI process or local-service restart. No automatic multi-font fallback, font packaging or text-layout expansion is implemented. See [text and progress](text-and-progress.md).

`preview --sheet` renders an evenly sampled PNG grid, using a count (default 12) or a positive `--interval` in rational seconds (exclusive end, maximum 128 thumbnails); `preview --boundaries` pairs output-grid frames around enabled visual item edges or one explicit time. Both reuse one PreviewEngine, return exact sample times and fixed revision metadata, and atomically publish under `cache/preview-sheets-v1/`. Boundary sampling uses the sequence frame rate, including rates above 24fps; missing start/end neighbors are explicit null cells. They inspect the final composition and do not automatically diagnose edit quality. See [preview modes and limits](preview.md#缩略图总览与边界帧).

## Local media diagnostics

`cutcraft doctor` runs outside project routing and reports FFmpeg/ffprobe launch/version checks, required encoder/filter availability, file-based graph support, and temporary PNG/FFV1/MP4 encoding plus MP4 probing/video/audio decoding. Per-check timeout defaults to 10 seconds (`--timeout 1..120`). Failed checks return a complete stdout JSON report with process exit 1; no project is opened or modified. This diagnoses the current CLI environment, not a previously started editor service or every possible source codec. See [doctor](doctor.md).

Export and MP4 previews accept `--video-encoder libx264|h264_videotoolbox|h264_nvenc|h264_qsv` and integer `--video-bitrate` (bits/s). Settings are runtime-only and forwarded through ApplicationHost, participate in MP4 preview cache keys, and are reported under `encoding` in result JSON. Hardware requests fail if unavailable; source decoding/compositing and interactive HLS remain CPU/software paths. See [encoding configuration](streaming-export.md#选择编码器).

`doctor --video-encoder NAME` tests the chosen adapter with actual H.264 encoding, frame probing and red/blue pixel decoding, using shared runtime encoder options and bounded checks. Default doctor remains software-only; listed adapters are not a promise of local hardware availability.

## 素材替换快捷入口

`asset replace ASSET_ID FILE` 复用 `asset.put`，支持普通媒体/图片/LUT、托管或外部存储、流映射、dry-run、撤销与 ApplicationHost 后台执行。保留素材/流身份和时间线，刷新新文件的真实元数据与指纹，拒绝源范围越界、并发目标变更、未经覆盖授权的锁定轨道。现行 0.1.0 与内部 0.3/0.4/0.5 共用已有协议，不新增持久化字段或版本。图片序列/字体及改变流拓扑仍需显式领域操作。详见[素材替换](search-and-locate.md#素材替换)。

## Sequence range cuts

`sequence cut` compiles an explicit all-track or selected-track interval into an atomic `sequence.put`, using the same exact slicing semantics as item edits. It supports gap-preserving and ripple removal, absolute-boundary LinkGroup partitioning, transition endpoint repair/removal, configurable marker cuts, fixed-duration adjustment, dry-run and undo. Unselected tracks are not implicitly edited; cross-scope links and invalid nested references reject. Partial loop/fade/legacy-automation slices remain explicitly unsupported; full removal/movement work. The planner materializes all IDs in the existing command schema, so no protocol/version change is required. See [range editing](range-edits.md).
