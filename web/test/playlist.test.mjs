import test from "node:test";
import assert from "node:assert/strict";
import { readPlaylist, PREVIEW_EXPIRED } from "../src/playlist.ts";
const playlist = (sequence, names, ended = false) =>
  `#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:${sequence}\n#EXT-X-MAP:URI="init.mp4"\n${names.map((name) => `#EXTINF:0.5,\n${name}`).join("\n")}\n${ended ? "#EXT-X-ENDLIST" : ""}`;
test("sliding playlists append only contiguous unseen fragments", () => {
  const result = readPlaylist(
    playlist(10, ["part-000010.m4s", "part-000011.m4s", "part-000012.m4s"]),
    12n,
  );
  assert.deepEqual(result.segments, [
    { path: "part-000012.m4s", sequence: 12n },
  ]);
  assert.equal(result.ended, false);
  assert.equal(
    readPlaylist(playlist(12, ["part-000012.m4s"], true), 13n).ended,
    true,
  );
});
test("initial and subsequent missing windows fail instead of skipping a time gap", () => {
  assert.throws(() => readPlaylist(playlist(6, ["part-000006.m4s"]), 0n), {
    message: PREVIEW_EXPIRED,
  });
  assert.throws(() => readPlaylist(playlist(30, ["part-000030.m4s"]), 29n), {
    message: PREVIEW_EXPIRED,
  });
  assert.throws(
    () => readPlaylist(playlist("bad", ["part-000000.m4s"]), 0n),
    /Invalid preview media sequence/,
  );
  assert.throws(
    () => readPlaylist(playlist(0, ["https://example.com/media"]), 0n),
    /Invalid preview fragment name/,
  );
});
