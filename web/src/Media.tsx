import { useEffect, useState } from "react";
import { request } from "./api";
import * as t from "./time";
const waveforms = new Map<string, Promise<any>>();
export function Thumbnail({
  handle,
  assetId,
  streamId,
  className = "",
}: {
  handle: string;
  assetId: string;
  streamId: string;
  className?: string;
}) {
  const [failed, setFailed] = useState(false);
  useEffect(() => setFailed(false), [handle, assetId, streamId]);
  if (failed) return null;
  return (
    <img
      loading="lazy"
      className={`media-thumbnail ${className}`}
      alt=""
      src={`/api/v1/projects/${handle}/assets/${assetId}/thumbnail?stream=${encodeURIComponent(streamId)}`}
      onError={() => setFailed(true)}
    />
  );
}
export function Waveform({
  handle,
  assetId,
  streamId,
  sourceRange,
  reverse,
}: {
  handle: string;
  assetId: string;
  streamId: string;
  sourceRange: any;
  reverse: boolean;
}) {
  const [wave, setWave] = useState<any>(null);
  useEffect(() => {
    let active = true;
    const key = `${handle}/${assetId}/${streamId}`;
    let pending = waveforms.get(key);
    if (!pending) {
      pending = request(
        `/projects/${handle}/assets/${assetId}/waveform?stream=${encodeURIComponent(streamId)}`,
      );
      waveforms.set(key, pending);
      pending.catch(() => waveforms.delete(key));
      if (waveforms.size > 128) {
        const first = waveforms.keys().next().value;
        if (first) waveforms.delete(first);
      }
    }
    pending
      .then((v) => {
        if (active) setWave(v);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [handle, assetId, streamId]);
  if (!wave?.peaks?.length || !sourceRange) return null;
  const length = t.seconds(t.from(wave.duration)),
    start = t.seconds(t.from(sourceRange.start)),
    duration = t.seconds(t.from(sourceRange.duration));
  if (!length || !duration) return null;
  const count = 128,
    points: string[] = [];
  for (let i = 0; i < count; i++) {
    const pos = reverse
      ? start + duration * (1 - i / (count - 1))
      : start + (duration * i) / (count - 1);
    const index = Math.min(
      wave.peaks.length - 1,
      Math.max(0, Math.floor((pos / length) * wave.peaks.length)),
    );
    const value = Number(wave.peaks[index]);
    if (!Number.isFinite(value)) return null;
    const y = Math.min(1, Math.abs(value)) * 12;
    points.push(`M ${i} ${14 - y} L ${i} ${14 + y}`);
  }
  return (
    <svg
      className="waveform"
      viewBox="0 0 128 28"
      preserveAspectRatio="none"
      aria-label="Audio waveform"
    >
      <path
        d={points.join(" ")}
        fill="none"
        stroke="currentColor"
        strokeWidth=".6"
      />
    </svg>
  );
}
