import { useEffect, useRef, useState } from "react";
import { ApiError, encode, request, requestId } from "./api";
import * as t from "./time";
import {
  readPlaylist,
  PREVIEW_EXPIRED,
  PREVIEW_WAIT_TIMEOUT_MS,
} from "./playlist";

export function Player({
  handle,
  sequence,
  revision,
  at,
  seekToken,
  onAt,
  onStopped,
  playing,
  onPlaying,
  onError,
  draft,
  scale,
}: {
  handle: string;
  sequence: string;
  revision: string;
  at: t.Rational;
  seekToken: number;
  onAt: (v: t.Rational) => void;
  onStopped: (v: t.Rational) => void;
  playing: boolean;
  onPlaying: (v: boolean) => void;
  onError: (e: unknown) => void;
  draft?: any;
  scale: number;
}) {
  const draftKey = draft ? encode(draft) : "";
  const video = useRef<HTMLVideoElement>(null),
    [image, setImage] = useState(""),
    [status, setStatus] = useState("Preparing frame…");
  const playbackClock = useRef<{
      origin: t.Rational;
      fps: t.Rational;
      end: t.Rational;
    } | null>(null),
    latest = useRef({ handle, sequence, at, seekToken }),
    currentImage = useRef(""),
    generation = useRef(0),
    clientId = useRef(requestId());
  latest.current = { handle, sequence, at, seekToken };
  const currentPlaybackPosition = () => {
    const clock = playbackClock.current;
    return clock && video.current
      ? t.playbackPosition(
          video.current.currentTime,
          clock.fps,
          clock.origin,
          clock.end,
        )
      : null;
  };
  useEffect(() => {
    onPlaying(false);
  }, [handle, sequence, revision, scale]);
  useEffect(() => {
    if (playing) return;
    const controller = new AbortController(),
      frameGeneration = ++generation.current;
    const timer = setTimeout(async () => {
      setStatus(draft ? "Rendering draft…" : "Rendering frame…");
      try {
        const r = await fetch(
          draft
            ? `/api/v1/projects/${handle}/preview/frame`
            : `/api/v1/projects/${handle}/preview/frame?sequence=${encodeURIComponent(sequence)}&at=${encodeURIComponent(t.exact(at))}&scale=${scale}&revision=${revision}&generation=${frameGeneration}&clientId=${clientId.current}`,
          draft
            ? {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: encode({
                  sequence,
                  scale,
                  at,
                  revision: BigInt(revision),
                  generation: frameGeneration,
                  clientId: clientId.current,
                  command: draft,
                }),
                signal: controller.signal,
              }
            : { signal: controller.signal },
        );
        if (!r.ok) {
          const body = await r.text();
          throw new Error(body || `Frame preview failed (${r.status}).`);
        }
        if (
          r.headers.get("X-Cutcraft-Revision") &&
          r.headers.get("X-Cutcraft-Revision") !== revision
        )
          throw new Error(
            "Preview revision changed. Refreshing the project is required.",
          );
        const url = URL.createObjectURL(await r.blob());
        if (
          controller.signal.aborted ||
          frameGeneration !== generation.current
        ) {
          URL.revokeObjectURL(url);
          return;
        }
        URL.revokeObjectURL(currentImage.current);
        currentImage.current = url;
        setImage(url);
        setStatus("");
      } catch (e) {
        if (!controller.signal.aborted) {
          setStatus(
            draft
              ? "Draft cannot be rendered at this position"
              : "Frame unavailable",
          );
          if (!draft) onError(e);
        }
      }
    }, 90);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [handle, sequence, revision, t.exact(at), playing, draftKey, scale]);
  useEffect(() => () => URL.revokeObjectURL(currentImage.current), []);
  useEffect(() => {
    if (!playing) return;
    playbackClock.current = null;
    const controller = new AbortController();
    let taskId = "",
      objectUrl = "";
    const wait = (ms: number) =>
      new Promise<void>((resolve) => {
        const finish = () => {
          clearTimeout(id);
          controller.signal.removeEventListener("abort", finish);
          resolve();
        };
        const id = setTimeout(finish, ms);
        controller.signal.addEventListener("abort", finish, { once: true });
      });
    const abort = () => {
      if (controller.signal.aborted)
        throw new DOMException("Aborted", "AbortError");
    };
    (async () => {
      setStatus("Buffering…");
      if (!("MediaSource" in window))
        throw new Error(
          "This browser does not support Media Source Extensions. Use a current desktop browser for playback.",
        );
      const job = await request(
        `/projects/${handle}/preview/playback`,
        {
          sequence,
          scale,
          start: at,
          revision: BigInt(revision),
          generation: ++generation.current,
          clientId: clientId.current,
        },
        controller.signal,
      );
      taskId = job.taskId;
      abort();
      let metadata: any;
      const metadataWaitStart = performance.now();
      while (!metadata) {
        try {
          metadata = await request(
            `/tasks/${taskId}/files/metadata.json`,
            undefined,
            controller.signal,
          );
        } catch (error) {
          if (!(error instanceof ApiError) || error.status !== 404) throw error;
          const { task } = await request(
            `/tasks/${taskId}`,
            undefined,
            controller.signal,
          );
          if (task.status === "failed" || task.status === "cancelled")
            throw new Error(task.error || "Playback stopped.");
          if (
            task.status === "completed" ||
            performance.now() - metadataWaitStart > PREVIEW_WAIT_TIMEOUT_MS
          )
            throw new Error(
              "Preview metadata is unavailable. Press Play to try again.",
            );
          await wait(150);
          abort();
        }
      }
      playbackClock.current = {
        origin: t.from(metadata.startTime),
        fps: t.from(metadata.frameRate),
        end: t.from(metadata.duration),
      };
      const source = new MediaSource();
      objectUrl = URL.createObjectURL(source);
      const el = video.current!;
      el.src = objectUrl;
      await new Promise<void>((resolve, reject) => {
        source.addEventListener("sourceopen", () => resolve(), { once: true });
        controller.signal.addEventListener(
          "abort",
          () => reject(new DOMException("Aborted", "AbortError")),
          { once: true },
        );
      });
      // Encoder advertises baseline H.264 / AAC; the decoder reads exact config from init.mp4.
      const mime = metadata.mime;
      if (!MediaSource.isTypeSupported(mime))
        throw new Error("H.264/AAC playback is unavailable in this browser.");
      const buffer = source.addSourceBuffer(mime);
      let nextSegment = 0n;
      const playlistWaitStart = performance.now();
      let initialized = false,
        playbackRequested = false;
      const append = async (path: string) => {
        const r = await fetch(
          `/api/v1/tasks/${taskId}/files/${encodeURIComponent(path)}`,
          { signal: controller.signal },
        );
        if (!r.ok)
          throw new Error(
            r.status === 404
              ? PREVIEW_EXPIRED
              : `Preview fragment unavailable (${r.status}).`,
          );
        const bytes = await r.arrayBuffer();
        abort();
        await new Promise<void>((resolve, reject) => {
          const done = () => {
            buffer.removeEventListener("error", failed);
            resolve();
          };
          const failed = () => {
            buffer.removeEventListener("updateend", done);
            reject(
              new Error("The browser could not decode a preview fragment."),
            );
          };
          buffer.addEventListener("updateend", done, { once: true });
          buffer.addEventListener("error", failed, { once: true });
          buffer.appendBuffer(bytes);
        });
      };
      while (!controller.signal.aborted) {
        const r = await fetch(`/api/v1/tasks/${taskId}/files/stream.m3u8`, {
          signal: controller.signal,
          cache: "no-store",
        });
        if (r.ok) {
          const manifest = readPlaylist(await r.text(), nextSegment);
          if (!initialized) {
            await append(manifest.init);
            initialized = true;
          }
          for (const segment of manifest.segments) {
            if (segment.sequence !== nextSegment)
              throw new Error(PREVIEW_EXPIRED);
            await append(segment.path);
            nextSegment = segment.sequence + 1n;
            if (!playbackRequested) {
              playbackRequested = true;
              // Do not await play: browsers may require the next fragment before
              // resolving this promise, so waiting here would deadlock buffering.
              void el.play().catch((error) => {
                if (!controller.signal.aborted) {
                  onError(error);
                  onPlaying(false);
                }
              });
            }
            // Keep memory bounded while leaving a small seek-back window.
            if (
              buffer.buffered.length &&
              el.currentTime > 15 &&
              buffer.buffered.start(0) < el.currentTime - 10
            ) {
              await new Promise<void>((resolve) => {
                buffer.addEventListener("updateend", () => resolve(), {
                  once: true,
                });
                buffer.remove(0, el.currentTime - 10);
              });
            }
            while (
              buffer.buffered.length &&
              buffer.buffered.end(buffer.buffered.length - 1) - el.currentTime >
                3
            ) {
              await wait(200);
              abort();
            }
          }
          if (manifest.ended) {
            if (source.readyState === "open") source.endOfStream();
            break;
          }
        } else if (r.status !== 404)
          throw new Error(`Playback failed (${r.status}).`);
        else if (initialized) throw new Error(PREVIEW_EXPIRED);
        const { task } = await request(
          `/tasks/${taskId}`,
          undefined,
          controller.signal,
        );
        if (task.status === "failed" || task.status === "cancelled")
          throw new Error(task.error || "Playback was cancelled.");
        if (
          !initialized &&
          (task.status === "completed" ||
            performance.now() - playlistWaitStart > PREVIEW_WAIT_TIMEOUT_MS)
        )
          throw new Error(
            "Preview stream is unavailable. Press Play to try again.",
          );
        await wait(250);
      }
    })().catch((e) => {
      if (!controller.signal.aborted) {
        setStatus("Playback unavailable");
        onPlaying(false);
        onError(e);
      }
    });
    return () => {
      controller.abort();
      const position = currentPlaybackPosition();
      // Preserve pause/end position, but an explicit seek or project switch wins.
      if (
        position &&
        latest.current.handle === handle &&
        latest.current.sequence === sequence &&
        latest.current.seekToken === seekToken &&
        t.compare(latest.current.at, at) === 0
      )
        onStopped(position);
      playbackClock.current = null;
      if (video.current) {
        video.current.pause();
        video.current.removeAttribute("src");
        video.current.load();
      }
      if (objectUrl) URL.revokeObjectURL(objectUrl);
      if (taskId) void request(`/tasks/${taskId}/cancel`, {}).catch(() => {});
    };
  }, [playing, handle, sequence, scale]);
  return (
    <div className="preview-surface">
      {image && (
        <img
          className={playing ? "hidden" : ""}
          src={image}
          alt="Rendered sequence at the playhead"
        />
      )}
      <video
        ref={video}
        className={!playing ? "hidden" : ""}
        playsInline
        onWaiting={() => setStatus("Buffering…")}
        onPlaying={() => setStatus("")}
        onError={() => {
          if (playing && video.current?.error) {
            onError(
              new Error(
                video.current.error.message ||
                  "The browser could not play this preview.",
              ),
            );
            onPlaying(false);
          }
        }}
        onEnded={() => {
          const position = currentPlaybackPosition();
          if (position) onStopped(position);
          onPlaying(false);
          setStatus("");
        }}
        onTimeUpdate={() => {
          if (playing) {
            const position = currentPlaybackPosition();
            if (position) onAt(position);
          }
        }}
      />
      {!image && !playing && (
        <div className="preview-empty">
          <span className="cut-mark">C</span>
          <span>Sequence preview</span>
        </div>
      )}
      {status && <span className="preview-status">{status}</span>}
    </div>
  );
}
