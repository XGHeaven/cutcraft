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
| Undo | New Operation applying inverses; undo of undo is redo | Inverse restore can require `--override-locks`; missing top-level optional metadata/extensions may restore as equivalent empty objects |
| Schema validation | Offline embedded JSON Schema 2020-12 | Embedded schema URI retains original Cutstate namespace |
| Semantic validation | Global IDs, graph cycles, kinds, bounds, overlap, links, crop, words, automation, handles | Custom effect/generator semantics are plugin-defined |
| Media validation | Paths, fingerprint, stream kind/index, actual source bounds, built-in parameter Schemas, unavailable-plugin rejection | Remote sources and font-family resolution are unsupported; no general plugin loader |
| Search and locate | Asset name/path/tag filters; media/LUT/font usage lookup; item/track/sequence discovery and parent-sequence references | Snapshot queries only; local sequence times; no opaque plugin-reference inference or content recognition. See [search and locate](search-and-locate.md). |
| Preview/export | Low-resolution MP4 preview; full-resolution MP4 export | Offline CPU rendering; no HTML player or live preview |
| Image sequences | Frozen manifests, printf/hash discovery, ordered file lists, atomic import/append, exact video sampling, frame-path search and paginated inspection | Local static images, max 1,000,000 slots; full import/media validation, on-demand decoding with a per-render 64 MiB cache; no migration/watch/VFR input manifests |

Text transparency and phased stderr progress are described in [text and progress](text-and-progress.md).

## Transaction and recovery

The public `edit::apply` is pure: it clones the snapshot, applies commands, normalizes canonical fields, validates the entire result, computes inverses, then returns the next snapshot and Event. Failure cannot mutate the caller's snapshot. Final validation happens after all batch members, allowing mutually dependent entities to be created together. Track locks are checked after **each** subcommand to prevent unlocking/relocking through a batch bypass.

The CLI holds a nonblocking exclusive OS advisory lock on `.cutcraft.lock` for snapshot access and editing. PID/acquiredAt/application are diagnostic metadata written only after acquiring the lock; the file is never replaced or deleted. Contention reports PROJECT_LOCKED. `Store` writes an fsynced journal containing the complete next snapshot and complete next history, replaces both public files using synced temporary files, then deletes the journal and syncs its directory. Opening the project recovers any remaining journal first. `.cutcraft/` must not be treated as disposable cache. This correctness-first implementation rewrites the history on commit and is O(history size); a future segmented WAL can improve large-history performance.

Rendering captures a validated revision and releases the editing lock before encoding. Its output describes that revision. Files are staged and published only after successful encoding, and existing output requires `--overwrite`. Source files can still be changed externally during rendering; managed assets and externally enforced immutability are preferred for reproducibility.

Preview produces only `preview.mp4` under project-root `cache/preview-<cli-version>-640-24-v1-<hash>-<sequence>/`. The reference renderer caps the long edge at 640 pixels and frame rate at 24 fps, retaining smaller dimensions/lower rates and rounding dimensions down to even numbers (minimum 2). Source buffers, effects and nested sequence canvases are reduced before composition; text sizes and margins are scaled with their owning canvas. H.264 uses ultrafast / CRF 28 and audio is capped at 24 kHz. Project format and final export settings are unchanged. Each invocation renders the complete selected sequence again; source decoding still scans complete inputs. The JSON result returns the MP4 path in `video` and `output`, with dimensions, frame rate and revision. There is no `--open` option or HTML page.

`cache/` contains only rebuildable data and is recreated on demand after deletion. Initialization creates `assets/`, `cache/` and `output/`; it no longer creates `working/`. Existing `working/` directories are left untouched because they may contain referenced media or editing inputs. Render staging remains temporary and is cleaned up after the render.

`undo` selects the latest applied Event, skipping rejected ones, and verifies both revision and snapshot hash. It does not delete history. Repeating undo toggles the most recent change (undo-of-undo), rather than walking a traditional GUI undo stack. Event inverses restore changed aggregates in full, including cascades and ripple edits. Lock overrides are external CLI capabilities, never project flags.

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

Followers' LinkGroups move together, once per operation. Ripple does not move sequence markers. Clips straddling an insertion boundary are not implicitly split; if the resulting state violates overlap policy the operation fails. Overwrite rejects linked or transition-connected competitors rather than leaving their synchronization or handles ambiguous. Item removal accepts only `none` and `remove-gap`.

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
