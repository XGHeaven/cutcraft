export const PREVIEW_EXPIRED =
  "Preview buffer expired. Press Play to restart at the current playhead.";
export const PREVIEW_WAIT_TIMEOUT_MS = 90_000;

/** A sliding playlist is contiguous only if it still contains our next segment. */
export function readPlaylist(text: string, nextSegment: bigint) {
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  const sequenceLine = lines.find((line) =>
    line.startsWith("#EXT-X-MEDIA-SEQUENCE:"),
  );
  const raw = sequenceLine?.slice("#EXT-X-MEDIA-SEQUENCE:".length) ?? "0";
  if (!/^\d+$/.test(raw)) throw new Error("Invalid preview media sequence.");
  const sequence = BigInt(raw);
  if (sequence > nextSegment) throw new Error(PREVIEW_EXPIRED);
  const init = text.match(/#EXT-X-MAP:URI="([^"]+)"/)?.[1] ?? "init.mp4";
  if (init !== "init.mp4")
    throw new Error("Invalid preview initialization fragment.");
  const segments = lines
    .filter((line) => !line.startsWith("#"))
    .map((path, index) => {
      if (!/^part-\d{6}\.m4s$/.test(path))
        throw new Error("Invalid preview fragment name.");
      return { path, sequence: sequence + BigInt(index) };
    })
    .filter((segment) => segment.sequence >= nextSegment);
  return { init, segments, ended: lines.includes("#EXT-X-ENDLIST") };
}
