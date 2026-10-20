---
name: cutcraft
description: Create, inspect, edit, preview, and render Cutcraft video projects, including multi-track video/audio, captions, nested sequences, motion, and color effects. Use when the user requests Cutcraft or wants local media edited into a Cutcraft project; not for developing the CLI itself.
---

# Cutcraft

Operate the installed Rust CLI to create a reviewable project and the requested video. This skill targets CLI/spec 0.1.0; query the installed binary rather than assuming every specification feature has a renderer.

## Available workflows

| User task | Implemented tools | Details to read when needed |
| --- | --- | --- |
| Set up or inspect a project; organize assets | init, inspect, probe, import, project/asset commands | [Editing guide: projects and media](references/editing-guide.md#projects-and-media) |
| Find an asset or locate its timeline uses | asset search/list/info/usages; item search/list/info; sequence/track list; sequence usages | [Editing guide: search and locate](references/editing-guide.md#search-and-locate) |
| Assemble clips and manage layers | sequence/track creation; item add/move/trim/split/remove; ripple and A/V links | [Editing guide: sequences, tracks and cuts](references/editing-guide.md#sequences-tracks-and-cuts) |
| Add music, mix sound or silence clips | Audio items, gain, pan, fades, mute/solo; silence generator | [Editing guide: audio](references/editing-guide.md#audio) |
| Add captions, titles, images or backgrounds | Caption items, text/color generators, image media, transforms | [Editing guide: text, images and backgrounds](references/editing-guide.md#text-images-and-backgrounds) |
| Import numbered or ordered image frames as video | import-sequence, asset frames, sample; video item editing and rendering | [Image sequence guide](references/image-sequences.md) |
| Reuse an edited segment or grade several layers | Nested sequences and video adjustment layers | [Editing guide: nested sequences and adjustment layers](references/editing-guide.md#nested-sequences-and-adjustment-layers) |
| Animate, retime, transition or color a clip | Property/effect animation, time maps, transitions, grade/curves/LUT | [Motion and effects](references/motion-and-effects.md) |
| Automate related edits; inspect or undo changes | command/apply, atomic batches, revisions, history/undo, markers | [Editing guide: domain commands and history](references/editing-guide.md#domain-commands-and-history) |
| Check and deliver an edit | Schema/semantic/media validation, sample, preview, render | [Editing guide: validation and delivery](references/editing-guide.md#validation-and-delivery) |

Read only the references relevant to the task. Some capabilities have dedicated CLI commands; others use domain JSON through `command`/`apply`. Do not invent a dedicated command for a field supported only in the project model.

## Locate the runtime and project

- Resolve `cutcraft` from PATH, a user-provided executable, or `target/release/cutcraft` / `target/debug/cutcraft` in an available Cutcraft checkout. This skill does not include the binary. Media work also requires `ffmpeg` and `ffprobe`; report missing dependencies when unavailable.
- When CLI installation is requested, use the repository's `scripts/install.sh`, available at `https://raw.githubusercontent.com/XGHeaven/cutcraft/main/scripts/install.sh`. The script is distributed from the main branch, independently of Releases; CLI binaries and SHA256SUMS still come from Releases. It defaults to the latest stable Release from `XGHeaven/cutcraft`. Use `--repo OWNER/REPO` only when another repository is supplied by the user or verified project configuration. Optional `--version VERSION`, `--bin-dir DIR` and `--force` select the version, destination and explicit replacement. The installer verifies SHA-256 and the binary version; FFmpeg and this skill remain separate installations.
- Read `cutcraft --version` and `cutcraft capabilities`. Use subcommand `--help` and `cutcraft schema project|operation|effect|animation|generator` for the installed contract. Do not guess flags or plugin availability.
- Honor the user's project/output location. For an existing project, inspect it first. For a new edit, create a new `.cut` project directory at the agreed location, not inside the skill installation or arbitrarily inside the source checkout.
- New snapshots are `cutcraft.json`. Do not silently rename legacy `project.json`, change specVersion, or recreate an existing project. Internal 0.3/0.4/0.5 projects retain compatibility. Image sequences require Spec 0.1.0 or internal 0.5.0; create a separate 0.1.0 project when using 0.3/0.4. No in-place upgrade command is installed.

## Assemble and edit

```sh
cutcraft init "$project" --name 'My edit'
cutcraft -p "$project" import "$clip" --managed
cutcraft -p "$project" inspect
```

`--managed` copies media into `assets/`; otherwise imports reference the original file. Probe or inspect stream metadata to select the actual video/audio stream IDs. If the color declaration is unknown, obtain the intended source space before setting `--color-space rec709|srgb|linear-srgb`; a filename or an unknown camera profile does not establish its transfer function. Do not relabel Log/HDR as Rec.709.

`import --append` makes tracks/items for that asset and links A/V. Repeated append is not a sequential montage on one track. For consecutive clips, create the desired tracks and add items with explicit timeline starts, durations and source ranges. Image cover streams are not the main video. Import and append are separate transactions, so inspect the result if append fails.

Use native discovery commands to resolve unknown IDs: `asset search` by name/path, `asset usages` to locate referring items, `item info` for the full editable item, and `sequence list` / `track list` for placement. Queries span all sequences; pass the returned `--sequence` when editing a non-root item. Use native item commands for ordinary edits. Read IDs and revisions from JSON output, rather than inventing them. Examples below assume those values have been resolved:

```sh
cutcraft -p "$project" track add --kind video --name 'V1'
cutcraft -p "$project" item add --track "$track_id" --asset "$asset_id" --stream "$stream_id" --start 0 --duration 2 --source-start 1 --source-duration 2
cutcraft -p "$project" item split "$item_id" --at 3/2
cutcraft -p "$project" item transform "$item_id" --scale-x 1.2 --scale-y 1.2
```

Timeline positions, including split `--at`, are sequence times. Animation offsets are local to their owner. CLI times accept seconds, decimal seconds and fractions; stored times are reduced `{num,den}` rational seconds, not frame counts or SMPTE strings.

For advanced fields and related changes, build a domain `command` or atomic `batch` and submit it using `command --base-revision REVISION` or a complete `apply` envelope. `item.put` replaces the entire item: clone the current value and preserve unrelated properties. Use `cutcraft id` to obtain new typed IDs. Prefer the CLI to editing `cutcraft.json` or history directly; no Python timeline-building step is required. Pass JSON using a file, a literal heredoc or subprocess stdin, not shell string interpolation.

- `--dry-run` validates edits without writing snapshot/history; it does not apply to init, import or render.
- On `REVISION_CONFLICT`, inspect again and recompute the intended edit. On ambiguous command completion, check history before repeating it; do not blindly retry a non-idempotent edit.
- Project locks and transaction recovery are already implemented. Do not delete `.cutcraft.lock` to bypass contention. Track-lock overrides require the user's intent to override that lock.
- Linked A/V edits may affect multiple items. Ripple defaults to `none`; select a ripple policy only when the task calls for moving followers.
- `undo` applies the inverse of the latest applied event. Repeating it toggles undo/redo rather than walking backward through the entire history.

For property/effect animation, speed, time maps, transitions and LUTs, read [references/motion-and-effects.md](references/motion-and-effects.md). It contains the actual JSON envelopes and important renderer boundaries.

## Validate and deliver

```sh
cutcraft -p "$project" validate --level media
cutcraft -p "$project" sample --at 1
cutcraft -p "$project" preview
cutcraft -p "$project" render --output output/final.mp4
```

Use `sample` to inspect source mappings and evaluated properties; it does not prove visual quality. Inspect representative output frames or play the exported video and verify timing, dimensions and audio as appropriate. Export paths are relative to the project unless absolute. Existing outputs require `--overwrite`; source media must not be overwritten.

Only claim effects that were actually rendered. The CPU reference renderer supports exact fit/remap/reverse/hold/loop sampling, frame blending, registered animations, built-in transitions, color/LUT effects and explicit audio resampling. Optical-flow, preserve-pitch and arbitrary plugin processors are not installed. It decodes media into temporary disk data and is intended for offline use; long/high-resolution work can be expensive. `preview` exports only a low-resolution MP4 of the entire selected sequence (max edge 640, max 24 fps, fast encoding), preserving the intended final format. Use the returned `video` path to view it.

Keep original/managed media in `assets/` or at its referenced location, generated final deliverables in `output/`, and rebuildable previews in project-root `cache/`. Do not put required media or animation input files in `cache/`. `.cutcraft/` and `history.jsonl` are persistence data, not cache. Existing `working/` folders may contain referenced files; leave them intact unless the task includes migration.

Return the project path and exported video path, with a brief account of the edit, checks and any unresolved limitation. Do not claim a video exists just because project edits succeeded.

For long renders/previews, use `--progress json` and consume stderr JSONL while preserving stdout for the final result. Native `item text` and styled `item caption` keep simple titles/subtitles editable; see [editing guide](references/editing-guide.md) for flags and layout limits.
