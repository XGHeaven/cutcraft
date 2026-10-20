# Image sequences (CLI/Spec 0.1.0)

Use `import-sequence` to register a frozen, ordered frame manifest as one video asset (`stream_v0`). Ordinary `import photo.png` remains a single still. Existing 0.3/0.4 projects return PROJECT_UPGRADE_REQUIRED; create a separate 0.1.0 project at the user-approved location rather than rewriting its version/history. Internal 0.5.0 projects still support image sequences. There is no in-place migration command.

```sh
cutcraft -p PROJECT import-sequence DIRECTORY --pattern 'frame_%04d.png' --fps 24 --color-space srgb --managed --append
cutcraft -p PROJECT import-sequence DIRECTORY --pattern 'frame_####.png' --pattern-style hash --fps 24 --color-space srgb
cutcraft -p PROJECT import-sequence DIRECTORY --files ordered-frames.json --fps 30000/1001 --color-space srgb --managed
```

- Use a known source color space; `--color-space` declares interpretation, not conversion. Unknown sources require an explicit declaration; conflicting known colors/dimensions fail.
- Default printf style supports exactly one `%d` or `%0Nd` slot and `%%` for a literal percent. Width is minimum zero padding: `%04d` accepts 0001 and 10000, not 1 or 00001. Hash style explicitly selects one digit per #; do not assume OpenImageIO's single-# four-digit behavior. Templates are basenames, not glob/regex; no recursion.
- `--fps` is required and uses rational seconds/rates. Numbering can begin at 1001; source time starts at zero. Source duration is frame count / frame rate, independent of project output fps.
- `--start-number` / `--end-number` are inclusive. Missing boundaries are inferred from matching candidates. `--missing error` is default; hold repeats the previous in-range frame, preserving duration (missing first frame fails); skip removes gaps, shortening duration. These options require pattern mode.
- `--files` consumes a JSON array of raw relative file paths, in exact order, with optional repeats/subdirectories. The JSON input file is relative to the CLI cwd; members are relative to DIRECTORY. It cannot be combined with pattern/range/style/missing options. The project stores encoded frame URIs and fingerprints, so the input list is no longer needed after import.
- `--managed` copies distinct files under assets/ASSET_ID/ and preserves original files. Otherwise the asset references its external directory. Import + optional append is one atomic batch; append places a full-duration video item at the root sequence end. Read IDs from the response. `--dry-run` is unsupported for imports.
- Directory additions never update the manifest. Missing/changed files after import fail validation/render; import gap policies are not ongoing repairs. Maximum 1,000,000 slots/range.

```sh
cutcraft -p PROJECT asset list --source-type image-sequence
cutcraft -p PROJECT asset search --path frame_0042.png
cutcraft -p PROJECT asset info ASSET_ID --summary
cutcraft -p PROJECT asset frames ASSET_ID --offset 0 --limit 100
cutcraft -p PROJECT asset usages ASSET_ID
cutcraft -p PROJECT item search frame_0042.png
cutcraft -p PROJECT sample --sequence SEQUENCE_ID --at 1/2
```

Frame pages use zero-based indices and limits 1–1000. `info --summary` avoids expanding a large manifest. Frame-path searches locate the owning asset/referring items, not a guarantee that each item actually samples that frame. `sample` adds imageSequenceFrames with indices, relative URIs and rational blend weights. Use video item sourceRange/retiming commands; do not expand a sequence into thousands of still items or invent a speed field.

Rendering preserves straight alpha and uses existing transforms, LUTs, animations, transitions, reverse, hold, loop and time-map sampling. Static PNG/JPEG/WebP/BMP/TIFF are supported; multi-image containers, remote files, automatic EXIF rotation, mixed dimensions and unsupported HDR/Log are rejected. Media validation checks all files; decoding is on demand with a per-render 64 MiB LRU cache. Preview keeps its low-resolution MP4 profile. Source files and manifests are not disposable cache; no watcher, automatic deduplication or persistent decode cache is installed.
