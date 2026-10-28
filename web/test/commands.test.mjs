import test from "node:test";
import assert from "node:assert/strict";
import { opacityCommand } from "../src/commands.ts";

test("opacity edits use video and preserve other properties without mutating the item", () => {
  const item = {
    id: "item_x",
    timelineRange: { start: { num: 0n, den: 1n } },
    video: { opacity: 0.8, transform: { position: { x: 10, y: 20 } } },
    extensions: { keep: true },
  };
  const before = structuredClone(item);
  const command = opacityCommand("sequence_x", "track_x", item, "0.25");
  assert.equal(command.item.video.opacity, 0.25);
  assert.equal("visual" in command.item, false);
  assert.deepEqual(command.item.video.transform, item.video.transform);
  assert.deepEqual(command.item.timelineRange, item.timelineRange);
  assert.deepEqual(command.item.extensions, item.extensions);
  assert.deepEqual(item, before);
  for (const bad of ["", "NaN", "-0.1", "1.1"])
    assert.throws(() => opacityCommand("s", "t", item, bad));
  assert.throws(() =>
    opacityCommand(
      "s",
      "t",
      { video: { opacity: { $animation: "curve" } } },
      "0.5",
    ),
  );
});
