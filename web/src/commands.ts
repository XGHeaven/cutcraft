import { entityId } from "./api";
import * as t from "./time";
export function trimCommand(
  sequenceId: string,
  item: any,
  start: t.Rational,
  duration: t.Rational,
) {
  if (t.compare(start, t.zero) < 0 || t.compare(duration, t.zero) <= 0)
    throw new Error("Start must be nonnegative and duration must be positive.");
  const command: any = {
    type: "item.trim",
    sequenceId,
    itemId: item.id,
    timelineRange: { start, duration },
    ripple: "none",
  };
  if (item.sourceRange && item.playback?.mode !== "remap") {
    if (item.playback?.mode && item.playback.mode !== "fit")
      throw new Error(
        "Trim loop and hold clips through the CLI with an explicit source range.",
      );
    const oldStart = t.from(item.timelineRange.start),
      oldDuration = t.from(item.timelineRange.duration);
    const rate = t.div(t.from(item.sourceRange.duration), oldDuration);
    const offset = item.playback?.reverse
      ? t.sub(t.add(oldStart, oldDuration), t.add(start, duration))
      : t.sub(start, oldStart);
    command.sourceRange = {
      start: t.add(t.from(item.sourceRange.start), t.mul(offset, rate)),
      duration: t.mul(duration, rate),
    };
  }
  return command;
}
export function appendAsset(
  sequenceId: string,
  sequence: any,
  assetId: string,
  asset: any,
) {
  let end = t.zero;
  if (sequence.durationMode === "fixed") end = t.from(sequence.duration);
  else {
    for (const track of sequence.tracks) {
      if (track.enabled === false) continue;
      for (const item of track.items) {
        if (item.enabled === false) continue;
        end = t.max(
          end,
          t.add(
            t.from(item.timelineRange.start),
            t.from(item.timelineRange.duration),
          ),
        );
      }
    }
    for (const marker of sequence.markers ?? [])
      end = t.max(end, t.add(t.from(marker.start), t.from(marker.duration)));
  }
  const commands: any[] = [],
    members: string[] = [];
  for (const stream of asset.streams ?? []) {
    if (
      stream.metadata?.role === "attached-picture" ||
      !["video", "image", "audio"].includes(stream.kind)
    )
      continue;
    const duration =
      stream.kind === "image"
        ? t.rational(5n)
        : stream.duration
          ? t.from(stream.duration)
          : null;
    if (!duration)
      throw new Error(
        "This stream has no known duration. Add it with an explicit range using the CLI.",
      );
    const item: any = {
      id: entityId("item"),
      type: "media",
      enabled: true,
      effects: [],
      assetId,
      streamId: stream.id,
      timelineRange: { start: end, duration },
    };
    if (stream.kind !== "image") item.sourceRange = { start: t.zero, duration };
    members.push(item.id);
    commands.push({
      type: "track.put",
      sequenceId,
      track: {
        id: entityId("track"),
        kind: stream.kind === "audio" ? "audio" : "video",
        name: `${asset.name ?? "Media"} ${stream.kind}`,
        overlapPolicy: "forbid",
        items: [item],
      },
    });
  }
  if (!commands.length)
    throw new Error("This asset has no playable audio or video streams.");
  if (members.length > 1) {
    const id = entityId("link");
    commands.push({
      type: "link.put",
      sequenceId,
      link: { id, kind: "av", itemIds: members },
    });
  }
  return { type: "batch", commands };
}
