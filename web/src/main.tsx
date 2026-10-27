import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  ApiError,
  authenticate,
  decode,
  entityId,
  request,
  requestId,
} from "./api";
import * as t from "./time";
import { appendAsset, trimCommand } from "./commands";
import { Player } from "./Player";
import { Icon, type IconName } from "./Icons";
import { Modal } from "./Modal";
import { Thumbnail, Waveform } from "./Media";
import "./style.css";

type ProjectInfo = {
  handle: string;
  path: string;
  name: string;
  revision: bigint;
};
type Drag = {
  id: string;
  mode: "move" | "left" | "right";
  originX: number;
  start: t.Rational;
  duration: t.Rational;
  nextStart: t.Rational;
  nextDuration: t.Rational;
  revision: bigint;
};
const icon = (name: IconName) => <Icon name={name} />;
function App() {
  const [ready, setReady] = useState(false),
    [projects, setProjects] = useState<ProjectInfo[]>([]),
    [handle, setHandle] = useState(
      new URLSearchParams(location.search).get("project") ??
        new URLSearchParams(location.hash.slice(1)).get("project") ??
        "",
    ),
    [snapshot, setSnapshot] = useState<any>(null),
    [tasks, setTasks] = useState<any[]>([]);
  const [sid, setSid] = useState(""),
    [selected, setSelected] = useState(""),
    [at, setAt] = useState(t.zero),
    [seekToken, setSeekToken] = useState(0),
    [displayAt, setDisplayAt] = useState(t.zero),
    [playing, setPlaying] = useState(false),
    [zoom, setZoom] = useState(70),
    [error, setError] = useState(""),
    [notice, setNotice] = useState(""),
    [pending, setPending] = useState(false),
    [connected, setConnected] = useState(false),
    [dialog, setDialog] = useState<
      "projects" | "import" | "export" | "tasks" | "properties" | null
    >(null),
    [previewScale, setPreviewScale] = useState(0.5);
  const [path, setPath] = useState(""),
    [importPath, setImportPath] = useState(""),
    [colorSpace, setColorSpace] = useState(""),
    [assetSearch, setAssetSearch] = useState(""),
    [exportPath, setExportPath] = useState("output/render.mp4"),
    [overwrite, setOverwrite] = useState(false),
    [drag, setDrag] = useState<Drag | null>(null);
  const dragRef = useRef<Drag | null>(null),
    currentHandle = useRef(handle),
    refreshSerial = useRef(0),
    [seekInput, setSeekInput] = useState("0");
  currentHandle.current = handle;
  const project = snapshot?.project,
    seq = project?.sequences?.[sid],
    fps = seq ? t.from(seq.format.frameRate) : t.rational(30n),
    tracks: any[] = seq?.tracks ?? [];
  const selectedTrack = tracks.find((track) =>
    track.items.some((item: any) => item.id === selected),
  );
  const item = selectedTrack?.items.find((it: any) => it.id === selected);
  let duration = t.rational(10n);
  for (const track of tracks)
    for (const clip of track.items)
      duration = t.max(
        duration,
        t.add(
          t.from(clip.timelineRange.start),
          t.from(clip.timelineRange.duration),
        ),
      );
  const width = Math.max(800, t.seconds(duration) * zoom + 140),
    activeProject = projects.find((p) => p.handle === handle);
  const report = (e: unknown) =>
    setError(e instanceof Error ? e.message : String(e));
  async function refresh(next = handle) {
    const serial = ++refreshSerial.current;
    const [list, jobs] = await Promise.all([
      request("/projects"),
      request("/tasks"),
    ]);
    if (serial !== refreshSerial.current) return;
    setProjects(list.projects ?? []);
    setTasks(jobs.tasks ?? []);
    const chosen = list.projects.some((p: ProjectInfo) => p.handle === next)
      ? next
      : (list.projects[0]?.handle ?? "");
    if (chosen !== currentHandle.current) {
      setHandle(chosen);
      setSnapshot(null);
      return;
    }
    if (chosen) {
      const snap = await request(`/projects/${chosen}/snapshot`);
      if (serial === refreshSerial.current && chosen === currentHandle.current)
        setSnapshot(snap);
    } else setSnapshot(null);
  }
  useEffect(() => {
    authenticate()
      .then(() => {
        setReady(true);
        return refresh();
      })
      .catch(report);
  }, []);
  useEffect(() => {
    if (!ready) return;
    setSnapshot(null);
    setSelected("");
    setPlaying(false);
    setAt(t.zero);
    setDisplayAt(t.zero);
    void refresh(handle).catch(report);
  }, [handle, ready]);
  useEffect(() => {
    if (!project) return;
    if (!project.sequences[sid]) setSid(project.rootSequenceId);
  }, [project, sid]);
  useEffect(() => {
    if (!ready) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const events = new EventSource("/api/v1/events");
    events.onopen = () => {
      setConnected(true);
      void refresh(currentHandle.current).catch(report);
    };
    events.onerror = () => setConnected(false);
    events.addEventListener("change", (event) => {
      const data = decode((event as MessageEvent).data);
      if (
        data.type === "project.closed" &&
        data.project === currentHandle.current
      ) {
        setPlaying(false);
        setSnapshot(null);
        setNotice("This project was closed from another client.");
      }
      if (data.type === "server.closing") {
        setConnected(false);
        setReady(false);
        setSnapshot(null);
        setNotice(
          "All projects are closed. The background service has stopped.",
        );
        events.close();
        clearTimeout(timer);
        return;
      }
      clearTimeout(timer);
      timer = setTimeout(
        () => void refresh(currentHandle.current).catch(report),
        80,
      );
    });
    return () => {
      clearTimeout(timer);
      events.close();
    };
  }, [ready]);
  const run = async (action: () => Promise<any>) => {
    if (pending) return;
    setPending(true);
    setError("");
    try {
      await action();
      await refresh(currentHandle.current);
    } catch (e) {
      report(e);
      if (e instanceof ApiError && e.status === 409) {
        setNotice(
          "The project changed. Latest revision loaded; your edit was not applied.",
        );
        await refresh(currentHandle.current).catch(report);
      }
    } finally {
      setPending(false);
    }
  };
  const domain = (command: any, baseRevision = project?.revision) =>
    request(`/projects/${handle}/commands`, {
      requestId: requestId(),
      baseRevision,
      command,
    });
  const cmd = (command: any) =>
    request(`/projects/${handle}/requests`, {
      requestId: requestId(),
      baseRevision: project?.revision,
      command,
    });
  const seek = (time: t.Rational) => {
    setSeekToken((value) => value + 1);
    setPlaying(false);
    setAt(t.max(t.zero, t.snap(time, fps)));
  };
  function startDrag(e: React.PointerEvent, clip: any, mode: Drag["mode"]) {
    if (pending || playing || e.button !== 0) return;
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    setSelected(clip.id);
    const start = t.from(clip.timelineRange.start),
      duration = t.from(clip.timelineRange.duration);
    const d = {
      id: clip.id,
      mode,
      originX: e.clientX,
      start,
      duration,
      nextStart: start,
      nextDuration: duration,
      revision: project.revision,
    };
    dragRef.current = d;
    setDrag(d);
  }
  function moveDrag(e: React.PointerEvent) {
    const d = dragRef.current;
    if (!d) return;
    const delta = t.pixelTime(e.clientX - d.originX, zoom, fps),
      frame = t.frameTime(1n, fps);
    let start = d.start,
      duration = d.duration;
    if (d.mode === "move") start = t.max(t.zero, t.add(d.start, delta));
    else if (d.mode === "left") {
      start = t.max(
        t.zero,
        t.min(t.sub(t.add(d.start, d.duration), frame), t.add(d.start, delta)),
      );
      duration = t.sub(t.add(d.start, d.duration), start);
    } else duration = t.max(frame, t.add(d.duration, delta));
    const next = { ...d, nextStart: start, nextDuration: duration };
    dragRef.current = next;
    setDrag(next);
  }
  function endDrag() {
    const d = dragRef.current;
    if (!d) return;
    dragRef.current = null;
    setDrag(null);
    if (
      t.compare(d.start, d.nextStart) === 0 &&
      t.compare(d.duration, d.nextDuration) === 0
    )
      return;
    void run(async () => {
      const original = tracks
        .flatMap((track) => track.items)
        .find((it) => it.id === d.id);
      const c =
        d.mode === "move"
          ? {
              type: "item.move",
              sequenceId: sid,
              itemId: d.id,
              timelineStart: d.nextStart,
              ripple: "none",
            }
          : trimCommand(sid, original, d.nextStart, d.nextDuration);
      await domain(c, d.revision);
    });
  }
  let draftCommand: any;
  if (drag) {
    const original = tracks
      .flatMap((track) => track.items)
      .find((it) => it.id === drag.id);
    if (
      original &&
      (t.compare(drag.start, drag.nextStart) !== 0 ||
        t.compare(drag.duration, drag.nextDuration) !== 0)
    ) {
      try {
        draftCommand =
          drag.mode === "move"
            ? {
                type: "item.move",
                sequenceId: sid,
                itemId: drag.id,
                timelineStart: drag.nextStart,
                ripple: "none",
              }
            : trimCommand(sid, original, drag.nextStart, drag.nextDuration);
      } catch {
        /* The release action reports invalid trim modes without writing. */
      }
    }
  }
  function split() {
    if (!item) return;
    void run(() =>
      domain({
        type: "item.split",
        sequenceId: sid,
        itemId: item.id,
        at,
        leftItemId: entityId("item"),
        rightItemId: entityId("item"),
      }),
    );
  }
  useEffect(() => {
    const shortcut = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (
        !seq ||
        dialog !== null ||
        pending ||
        target?.matches("input, textarea, select") ||
        target?.isContentEditable
      )
        return;
      const mod = event.ctrlKey || event.metaKey;
      if (mod && event.key.toLowerCase() === "z") {
        event.preventDefault();
        const action = event.shiftKey ? "redo" : "undo";
        if (
          action === "undo"
            ? snapshot?.history?.canUndo
            : snapshot?.history?.canRedo
        )
          void run(() =>
            request(`/projects/${handle}/history/${action}`, {
              requestId: requestId(),
              baseRevision: project.revision,
            }),
          );
      } else if (event.code === "Space" && !mod) {
        event.preventDefault();
        setDisplayAt(at);
        setPlaying(!playing);
      } else if (mod && event.key.toLowerCase() === "b" && item && !playing) {
        event.preventDefault();
        split();
      } else if (
        !mod &&
        !playing &&
        (event.key === "ArrowLeft" || event.key === "ArrowRight")
      ) {
        event.preventDefault();
        seek(t.add(at, t.frameTime(event.key === "ArrowLeft" ? -1n : 1n, fps)));
      }
    };
    window.addEventListener("keydown", shortcut);
    return () => window.removeEventListener("keydown", shortcut);
  }, [
    pending,
    handle,
    project?.revision,
    selected,
    playing,
    t.exact(at),
    sid,
    dialog,
  ]);
  const openProject = (e: React.FormEvent) => {
    e.preventDefault();
    void run(async () => {
      const opened = await request("/projects/open", { path });
      setPath("");
      const p = opened.project ?? opened;
      if (p.handle) setHandle(p.handle);
      setDialog(null);
    });
  };
  async function closeProject(p: ProjectInfo) {
    await request(`/projects/${p.handle}/close`, {});
    setProjects((old) => old.filter((x) => x.handle !== p.handle));
    if (p.handle === handle) {
      setSnapshot(null);
      setHandle("");
    }
    if (projects.length === 1) {
      setReady(false);
      setNotice(
        "Project closed. The background service has stopped. Run cutcraft open to start it again.",
      );
    }
  }
  const library = (
    <aside className="library panel">
      <div className="panel-title">
        <span>Media library</span>
        <button
          className="library-import"
          disabled={!project}
          onClick={() => setDialog("import")}
          aria-label="Import media"
        >
          {icon("add")} Import
        </button>
      </div>
      <div className="library-tools">
        <input
          aria-label="Search media"
          placeholder="Search media…"
          value={assetSearch}
          onChange={(e) => setAssetSearch(e.target.value)}
        />
      </div>
      <div className="asset-list">
        {Object.entries<any>(project?.assets ?? {})
          .filter(([id, a]) =>
            `${a.name} ${id}`.toLowerCase().includes(assetSearch.toLowerCase()),
          )
          .map(([id, a]) => (
            <div className="asset" key={id}>
              <div
                className={`asset-art ${a.streams?.some((s: any) => s.kind === "video" || s.kind === "image") ? "visual" : "sound"}`}
              >
                {icon(
                  a.streams?.some(
                    (s: any) => s.kind === "video" || s.kind === "image",
                  )
                    ? "media"
                    : "audio",
                )}
                {a.streams?.find(
                  (s: any) => s.kind === "video" || s.kind === "image",
                ) && (
                  <Thumbnail
                    handle={handle}
                    assetId={id}
                    streamId={
                      a.streams.find(
                        (s: any) => s.kind === "video" || s.kind === "image",
                      ).id
                    }
                  />
                )}
              </div>
              <div className="asset-info">
                <strong title={a.name ?? id}>{a.name ?? id}</strong>
                <small>
                  {a.streams?.map((s: any) => s.kind).join(" · ") ?? a.kind}
                </small>
              </div>
              <button
                title="Append to sequence"
                aria-label={`Append ${a.name ?? id}`}
                disabled={pending || !seq}
                onClick={() =>
                  void run(() => domain(appendAsset(sid, seq, id, a)))
                }
              >
                {icon("add")}
              </button>
            </div>
          ))}
        {!Object.keys(project?.assets ?? {}).length && (
          <div className="empty">
            <span>{icon("media")}</span>
            <strong>Your media starts here</strong>
            <p>Import a local file, then add it to the timeline.</p>
          </div>
        )}
      </div>
      <div className="library-footer">
        {activeProject?.path ?? "No project open"}
      </div>
    </aside>
  );
  return (
    <div className="app">
      <header className="topbar">
        <a className="brand" href="/" aria-label="Cutcraft editor">
          <span className="logo">{icon("logo")}</span>
          <span className="brand-name">cutcraft</span>
        </a>
        <button
          className="project-title"
          onClick={() => setDialog("projects")}
          aria-label="Manage projects"
          aria-haspopup="dialog"
          aria-expanded={dialog === "projects"}
          title={project?.name ?? "Open a project"}
        >
          {icon("folder")}
          <span className="project-name">
            {project?.name ?? "Open a project"}
          </span>
          {project && (
            <small className="revision">rev {String(project.revision)}</small>
          )}
          {icon("chevron")}
        </button>
        <div className="top-actions">
          <span className={`connection ${connected ? "online" : ""}`}>
            <i />
            {connected ? "Connected" : "Disconnected"}
          </span>
          <button
            className="quiet"
            aria-label="Background tasks"
            title="Background tasks"
            onClick={() => setDialog("tasks")}
          >
            {icon("tasks")} <span className="action-label">Tasks</span>
            {tasks.some(
              (task) =>
                task.project === handle &&
                task.kind !== "preview" &&
                ["running", "queued"].includes(task.status),
            )
              ? " •"
              : ""}
          </button>
          <button
            className="primary"
            aria-label="Export video"
            title="Export video"
            disabled={!project}
            onClick={() => setDialog("export")}
          >
            {icon("export")} <span className="action-label">Export</span>
          </button>
        </div>
      </header>
      {error && (
        <div className="banner error" role="alert">
          <span>{error}</span>
          <button onClick={() => setError("")} aria-label="Dismiss error">
            {icon("close")}
          </button>
        </div>
      )}
      {notice && (
        <div className="banner notice" role="status">
          <span>{notice}</span>
          <button
            onClick={() => setNotice("")}
            aria-label="Dismiss notification"
          >
            {icon("close")}
          </button>
        </div>
      )}
      <main className="workspace">
        {library}
        <section className="viewer panel">
          <div className="panel-title">
            <span className="monitor-title">Program monitor</span>
            <select
              aria-label="Sequence"
              value={sid}
              onChange={(e) => {
                setSid(e.target.value);
                setSelected("");
                seek(t.zero);
              }}
            >
              {Object.entries<any>(project?.sequences ?? {}).map(([id, s]) => (
                <option value={id} key={id}>
                  {s.name ?? id}
                </option>
              ))}
            </select>
            <small>
              {seq
                ? `${seq.format.width} × ${seq.format.height} · ${t.exact(fps)} fps`
                : ""}
            </small>
          </div>
          {seq ? (
            <Player
              handle={handle}
              sequence={sid}
              revision={String(project.revision)}
              at={at}
              seekToken={seekToken}
              onAt={setDisplayAt}
              onStopped={(time) => {
                setAt(time);
                setDisplayAt(time);
              }}
              playing={playing}
              onPlaying={setPlaying}
              onError={report}
              draft={draftCommand}
              scale={previewScale}
            />
          ) : (
            <div className="preview-surface">
              <div className="preview-empty">
                <span className="cut-mark">C</span>
                <span>Open a project to start editing</span>
              </div>
            </div>
          )}
          <div className="transport">
            <span className="timecode">
              {t.display(playing ? displayAt : at)}
            </span>
            <div className="transport-buttons">
              <button
                title="Previous frame"
                aria-label="Previous frame"
                disabled={!seq || playing}
                onClick={() => seek(t.sub(at, t.frameTime(1n, fps)))}
              >
                {icon("previous")}
              </button>
              <button
                className="play"
                title={playing ? "Pause" : "Play"}
                aria-label={playing ? "Pause" : "Play"}
                disabled={!seq}
                onClick={() => {
                  setDisplayAt(at);
                  setPlaying(!playing);
                }}
              >
                {icon(playing ? "pause" : "play")}
              </button>
              <button
                title="Next frame"
                aria-label="Next frame"
                disabled={!seq || playing}
                onClick={() => seek(t.add(at, t.frameTime(1n, fps)))}
              >
                {icon("next")}
              </button>
            </div>
            <label className="preview-quality">
              <select
                aria-label="Preview resolution"
                value={previewScale}
                onChange={(e) => {
                  setPlaying(false);
                  setPreviewScale(Number(e.target.value));
                }}
              >
                <option value={1}>1× · Full</option>
                <option value={0.5}>0.5× · Half</option>
                <option value={0.25}>0.25× · Quarter</option>
              </select>
            </label>
          </div>
        </section>

        <section className="timeline panel">
          <div className="timeline-toolbar">
            <span className="sequence-label">
              {icon("track")} <span>{seq?.name ?? "Timeline"}</span>
            </span>
            <div className="history-buttons">
              <button
                title="Undo (Ctrl/Cmd+Z)"
                aria-label="Undo"
                disabled={pending || !snapshot?.history?.canUndo}
                onClick={() =>
                  void run(() =>
                    request(`/projects/${handle}/history/undo`, {
                      requestId: requestId(),
                      baseRevision: project.revision,
                    }),
                  )
                }
              >
                {icon("undo")}
              </button>
              <button
                title="Redo (Ctrl/Cmd+Shift+Z)"
                aria-label="Redo"
                disabled={pending || !snapshot?.history?.canRedo}
                onClick={() =>
                  void run(() =>
                    request(`/projects/${handle}/history/redo`, {
                      requestId: requestId(),
                      baseRevision: project.revision,
                    }),
                  )
                }
              >
                {icon("redo")}
              </button>
            </div>
            <button disabled={!item} onClick={() => setDialog("properties")}>
              {icon("settings")} Properties
            </button>
            <button disabled={!item || pending || playing} onClick={split}>
              {icon("split")} Split
            </button>
            <button
              disabled={!seq || pending}
              onClick={() =>
                void run(() =>
                  domain({
                    type: "track.put",
                    sequenceId: sid,
                    track: {
                      id: entityId("track"),
                      name: "Video track",
                      kind: "video",
                      overlapPolicy: "forbid",
                      items: [],
                    },
                  }),
                )
              }
            >
              {icon("add")} Track
            </button>
            <form
              className="seek-form"
              onSubmit={(e) => {
                e.preventDefault();
                try {
                  seek(t.parseTime(seekInput));
                } catch (err) {
                  report(err);
                }
              }}
            >
              <input
                aria-label="Go to exact time in seconds"
                value={seekInput}
                onChange={(e) => setSeekInput(e.target.value)}
                placeholder="Seconds / fraction"
              />
              <button disabled={!seq}>Go</button>
            </form>
            <label className="zoom">
              {icon("minus")}
              <input
                aria-label="Timeline zoom"
                type="range"
                min="20"
                max="180"
                value={zoom}
                onChange={(e) => setZoom(Number(e.target.value))}
              />
              {icon("add")}
            </label>
          </div>
          <div className="timeline-scroll">
            <div className="timeline-inner" style={{ width: width + 168 }}>
              <div className="ruler-row">
                <div className="track-heading">TRACKS</div>
                <div
                  className="ruler"
                  style={{ width }}
                  onPointerDown={(e) => {
                    if (e.button === 0)
                      seek(
                        t.pixelTime(
                          e.clientX -
                            e.currentTarget.getBoundingClientRect().left,
                          zoom,
                          fps,
                        ),
                      );
                  }}
                >
                  {Array.from(
                    {
                      length:
                        Math.ceil(t.seconds(duration) / (zoom < 40 ? 5 : 1)) +
                        2,
                    },
                    (_, i) => {
                      const sec = i * (zoom < 40 ? 5 : 1);
                      return (
                        <span key={i} style={{ left: sec * zoom }}>
                          {Math.floor(sec / 60)}:
                          {String(sec % 60).padStart(2, "0")}
                        </span>
                      );
                    },
                  )}
                </div>
              </div>
              {tracks.map((track, index) => (
                <div className={`track-row ${track.kind}`} key={track.id}>
                  <div className="track-heading">
                    <strong>
                      <span>
                        {track.kind === "audio" ? "A" : "V"}
                        {index + 1}
                      </span>
                      {track.name ?? track.kind}
                    </strong>
                    <div>
                      <button
                        title={
                          track.enabled === false
                            ? "Enable track"
                            : "Disable track"
                        }
                        aria-label={`${track.enabled === false ? "Enable" : "Disable"} ${track.name ?? track.kind}`}
                        disabled={pending}
                        onClick={() =>
                          void run(() =>
                            domain({
                              type: "track.put",
                              sequenceId: sid,
                              track: {
                                ...track,
                                enabled: track.enabled === false,
                              },
                            }),
                          )
                        }
                      >
                        {icon(track.enabled === false ? "eyeOff" : "eye")}
                      </button>
                      <span
                        title={track.locked ? "Locked track" : "Unlocked track"}
                      >
                        {track.locked ? icon("lock") : null}
                      </span>
                      <small>{track.kind}</small>
                    </div>
                  </div>
                  <div
                    className="track-lane"
                    style={{ width }}
                    onPointerDown={(e) => {
                      if (e.target === e.currentTarget) {
                        setSelected("");
                        seek(
                          t.pixelTime(
                            e.clientX -
                              e.currentTarget.getBoundingClientRect().left,
                            zoom,
                            fps,
                          ),
                        );
                      }
                    }}
                  >
                    {track.items.map((clip: any) => {
                      const d = drag?.id === clip.id ? drag : null,
                        start =
                          d?.nextStart ?? t.from(clip.timelineRange.start),
                        duration =
                          d?.nextDuration ??
                          t.from(clip.timelineRange.duration);
                      const asset = project.assets[clip.assetId];
                      return (
                        <div
                          role="button"
                          tabIndex={0}
                          aria-label={`Clip ${clip.name ?? asset?.name ?? clip.type}`}
                          className={`clip ${selected === clip.id ? "selected" : ""} ${clip.enabled === false ? "disabled" : ""}`}
                          style={{
                            left: t.seconds(start) * zoom,
                            width: Math.max(8, t.seconds(duration) * zoom),
                          }}
                          key={clip.id}
                          onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ")
                              setSelected(clip.id);
                          }}
                          onDoubleClick={() => {
                            setSelected(clip.id);
                            setDialog("properties");
                          }}
                          onPointerDown={(e) => startDrag(e, clip, "move")}
                          onPointerMove={moveDrag}
                          onPointerUp={endDrag}
                          onPointerCancel={() => {
                            dragRef.current = null;
                            setDrag(null);
                          }}
                        >
                          <div
                            className="trim-handle left"
                            title="Trim start"
                            onPointerDown={(e) => startDrag(e, clip, "left")}
                          />
                          <span className="clip-name">
                            {clip.name ??
                              asset?.name ??
                              clip.text ??
                              clip.parameters?.text ??
                              clip.generatorType ??
                              clip.type}
                          </span>
                          <span className="clip-duration">
                            {t.display(duration)}
                          </span>
                          {clip.assetId &&
                            clip.streamId &&
                            track.kind === "video" && (
                              <Thumbnail
                                className="clip-thumbnail"
                                handle={handle}
                                assetId={clip.assetId}
                                streamId={clip.streamId}
                              />
                            )}
                          {clip.assetId &&
                            clip.streamId &&
                            track.kind === "audio" &&
                            (!clip.playback?.mode ||
                              clip.playback?.mode === "fit") && (
                              <Waveform
                                handle={handle}
                                assetId={clip.assetId}
                                streamId={clip.streamId}
                                sourceRange={clip.sourceRange}
                                reverse={clip.playback?.reverse === true}
                              />
                            )}
                          <div
                            className="trim-handle right"
                            title="Trim end"
                            onPointerDown={(e) => startDrag(e, clip, "right")}
                          />
                        </div>
                      );
                    })}
                  </div>
                </div>
              ))}
              {!tracks.length && (
                <div className="timeline-empty">
                  Add media from the library or create a track to begin.
                </div>
              )}
              {seq && (
                <div
                  className="playhead"
                  style={{
                    left: 168 + t.seconds(playing ? displayAt : at) * zoom,
                  }}
                >
                  <i />
                </div>
              )}
            </div>
          </div>
          <div className="timeline-footer">
            <span>
              {tracks.length} tracks ·{" "}
              {tracks.reduce((n, tr) => n + tr.items.length, 0)} clips
            </span>
            <span>
              {drag
                ? "Release to apply one edit"
                : selected
                  ? "Drag to move · Drag edges to trim"
                  : "Click the ruler to seek · Select a clip to edit"}
            </span>
            <span>Exact position {t.exact(at)} s</span>
          </div>
        </section>
      </main>
      {dialog && (
        <Modal
          title={
            {
              projects: "Projects",
              import: "Import media",
              export: "Export video",
              tasks: "Background tasks",
              properties: "Clip properties",
            }[dialog]
          }
          onClose={() => setDialog(null)}
        >
          {error && (
            <div className="banner error" role="alert">
              {error}
            </div>
          )}
          {dialog === "projects" && (
            <>
              <form className="stack-form" onSubmit={openProject}>
                <label>
                  Project directory
                  <input
                    value={path}
                    onChange={(e) => setPath(e.target.value)}
                    placeholder="/path/to/project.cut"
                    required
                  />
                </label>
                <button className="primary" disabled={pending || !ready}>
                  Open project
                </button>
              </form>
              <div className="project-list">
                {projects.map((p) => (
                  <div
                    className={`project-row ${handle === p.handle ? "selected" : ""}`}
                    key={p.handle}
                  >
                    <button
                      onClick={() => {
                        setHandle(p.handle);
                        setDialog(null);
                      }}
                    >
                      <strong>{p.name}</strong>
                      <small title={p.path}>{p.path}</small>
                    </button>
                    <button
                      aria-label={`Close ${p.name}`}
                      disabled={pending}
                      onClick={() => {
                        setPending(true);
                        setError("");
                        closeProject(p)
                          .catch(report)
                          .finally(() => setPending(false));
                      }}
                    >
                      {icon("close")}
                    </button>
                  </div>
                ))}
              </div>
            </>
          )}
          {dialog === "import" && (
            <>
              <form
                className="stack-form import-form"
                onSubmit={(e) => {
                  e.preventDefault();
                  void run(async () => {
                    await cmd({
                      Import: {
                        file: importPath,
                        color_space: colorSpace || null,
                        managed: false,
                        append: false,
                      },
                    });
                    setImportPath("");
                    setDialog("tasks");
                  });
                }}
              >
                <label>
                  Import from this computer
                  <input
                    value={importPath}
                    onChange={(e) => setImportPath(e.target.value)}
                    placeholder="/path/to/video.mp4"
                    required
                  />
                </label>
                <div className="form-row">
                  <select
                    aria-label="Source color space"
                    value={colorSpace}
                    onChange={(e) => setColorSpace(e.target.value)}
                  >
                    <option value="">Use media color tags</option>
                    <option value="rec709">Rec.709</option>
                    <option value="srgb">sRGB</option>
                    <option value="linear-srgb">Linear sRGB</option>
                  </select>
                  <button disabled={pending || !project}>Import</button>
                </div>
              </form>
            </>
          )}
          {dialog === "export" && (
            <>
              <section className="export-panel" id="export-panel">
                <h3>Export</h3>
                <form
                  className="stack-form"
                  onSubmit={(e) => {
                    e.preventDefault();
                    void run(async () => {
                      await cmd({
                        Render: {
                          sequence: sid,
                          output: exportPath,
                          overwrite,
                          progress: "Json",
                        },
                      });
                      setDialog("tasks");
                    });
                  }}
                >
                  <label>
                    Output path
                    <input
                      value={exportPath}
                      onChange={(e) => setExportPath(e.target.value)}
                      required
                    />
                  </label>
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={overwrite}
                      onChange={(e) => setOverwrite(e.target.checked)}
                    />
                    Replace an existing output
                  </label>
                  <button className="primary" disabled={!seq || pending}>
                    {icon("export")} Export MP4
                  </button>
                </form>
              </section>
            </>
          )}
          {dialog === "tasks" && (
            <>
              <section className="tasks">
                <h3>
                  Tasks{" "}
                  <small>
                    {
                      tasks.filter(
                        (task) =>
                          task.project === handle && task.kind !== "preview",
                      ).length
                    }
                  </small>
                </h3>
                {tasks
                  .filter(
                    (task) =>
                      task.project === handle && task.kind !== "preview",
                  )
                  .map((task) => (
                    <div className="task" key={task.id}>
                      <div>
                        <strong>{task.kind}</strong>
                        <span>{task.status}</span>
                      </div>
                      {task.progress && (
                        <small>
                          {typeof task.progress === "string"
                            ? task.progress
                            : (task.progress.stage ?? "Processing…")}
                        </small>
                      )}
                      {task.error && (
                        <p className="task-error">
                          {typeof task.error === "string"
                            ? task.error
                            : task.error.message}
                        </p>
                      )}
                      {["running", "queued"].includes(task.status) && (
                        <button
                          onClick={() =>
                            void run(() =>
                              request(`/tasks/${task.id}/cancel`, {}),
                            )
                          }
                        >
                          Cancel
                        </button>
                      )}
                    </div>
                  ))}
              </section>
              {!tasks.some(
                (task) => task.project === handle && task.kind !== "preview",
              ) && <p className="empty">No background tasks.</p>}
            </>
          )}
          {dialog === "properties" && (
            <>
              {item ? (
                <Inspector
                  key={`${item.id}:${project.revision}`}
                  item={item}
                  track={selectedTrack}
                  tracks={tracks}
                  sequenceId={sid}
                  disabled={pending || playing}
                  commit={(c) => void run(() => domain(c))}
                  report={report}
                />
              ) : (
                <div className="empty compact">
                  <span>{icon("settings")}</span>
                  <strong>Select a clip</strong>
                  <p>Adjust its timing, track, and properties here.</p>
                </div>
              )}
            </>
          )}
        </Modal>
      )}
    </div>
  );
}
function Inspector({
  item,
  track,
  tracks,
  sequenceId,
  disabled,
  commit,
  report,
}: {
  item: any;
  track: any;
  tracks: any[];
  sequenceId: string;
  disabled: boolean;
  commit: (c: any) => void;
  report: (e: unknown) => void;
}) {
  const [start, setStart] = useState(t.exact(t.from(item.timelineRange.start))),
    [duration, setDuration] = useState(
      t.exact(t.from(item.timelineRange.duration)),
    ),
    [name, setName] = useState(item.name ?? ""),
    [target, setTarget] = useState(track.id),
    [opacity, setOpacity] = useState(
      !!item.visual?.opacity?.$animation
        ? ""
        : String(item.visual?.opacity ?? 1),
    );
  const attempt = (f: () => any) => {
    try {
      commit(f());
    } catch (e) {
      report(e);
    }
  };
  return (
    <div className="properties">
      <div className="property-caption">
        {item.type}
        <small title={item.id}>{item.id}</small>
      </div>
      <form
        className="stack-form"
        onSubmit={(e) => {
          e.preventDefault();
          attempt(() => ({
            type: "item.put",
            sequenceId,
            trackId: track.id,
            item: { ...item, name },
          }));
        }}
      >
        <label>
          Clip name
          <input value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <button disabled={disabled}>Rename</button>
      </form>
      <form
        className="stack-form"
        onSubmit={(e) => {
          e.preventDefault();
          attempt(() =>
            trimCommand(
              sequenceId,
              item,
              t.parseTime(start),
              t.parseTime(duration),
            ),
          );
        }}
      >
        <div className="form-row">
          <label>
            Start (seconds)
            <input
              value={start}
              onChange={(e) => setStart(e.target.value)}
              required
            />
          </label>
          <label>
            Duration
            <input
              value={duration}
              onChange={(e) => setDuration(e.target.value)}
              required
            />
          </label>
        </div>
        <button disabled={disabled}>Apply timing</button>
      </form>
      <form
        className="stack-form"
        onSubmit={(e) => {
          e.preventDefault();
          attempt(() => ({
            type: "item.move",
            sequenceId,
            itemId: item.id,
            toTrackId: target,
            ripple: "none",
          }));
        }}
      >
        <label>
          Track
          <select value={target} onChange={(e) => setTarget(e.target.value)}>
            {tracks
              .filter((tr) => tr.kind === track.kind)
              .map((tr) => (
                <option key={tr.id} value={tr.id}>
                  {tr.name ?? tr.id}
                </option>
              ))}
          </select>
        </label>
        <button disabled={disabled || target === track.id}>
          Move to track
        </button>
      </form>
      {track.kind === "video" && (
        <form
          className="stack-form"
          onSubmit={(e) => {
            e.preventDefault();
            attempt(() => {
              const value = Number(opacity);
              if (
                opacity.trim() === "" ||
                !Number.isFinite(value) ||
                value < 0 ||
                value > 1
              )
                throw new Error("Opacity must be between 0 and 1.");
              return {
                type: "item.put",
                sequenceId,
                trackId: track.id,
                item: { ...item, visual: { ...item.visual, opacity: value } },
              };
            });
          }}
        >
          <label>
            Opacity
            <input
              type="number"
              min="0"
              max="1"
              step="0.01"
              value={opacity}
              placeholder="Animated"
              disabled={!!item.visual?.opacity?.$animation}
              onChange={(e) => setOpacity(e.target.value)}
            />
          </label>
          <button disabled={disabled || opacity === ""}>Apply opacity</button>
        </form>
      )}
      <label className="check">
        <input
          type="checkbox"
          checked={item.enabled !== false}
          disabled={disabled}
          onChange={(e) =>
            commit({
              type: "item.put",
              sequenceId,
              trackId: track.id,
              item: { ...item, enabled: e.target.checked },
            })
          }
        />
        Clip enabled
      </label>
      <button
        className="danger"
        disabled={disabled}
        onClick={() =>
          commit({
            type: "item.remove",
            sequenceId,
            itemId: item.id,
            ripple: "none",
            cascade: true,
          })
        }
      >
        Remove clip
      </button>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
