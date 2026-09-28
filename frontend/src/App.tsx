/**
 * TheTrimmer's window: one video, two marks, one button.
 *
 * ## What this is
 *
 * The form the product has always been — pick a master, type where the segment starts and
 * ends, trim — with the compartments a broadcast panel would have. There is **no queue**:
 * the window holds one range at a time, because a range is the whole of the decision and
 * a list of them was a second thing to understand in front of the first.
 *
 * ## The frame
 *
 * ```
 * ┌ TheTrimmer ─ frame-exact, lossless ───────────────────────────────────  Fullscreen  Light ┐
 * ├───────────────────────────────┬──────────────────────────────────────────────────────────┤
 * │ VIDEO                         │ WHAT CAME OUT                                            │
 * │ CAPTION FILE                  │   a verdict per check, measured against the source       │
 * │ RANGE · In · Out · the plan   │                                                          │
 * │ OPTIONS · delivery · verify   │                                                          │
 * │ ── what will happen ── [Trim] │                                                          │
 * ├───────────────────────────────┴──────────────────────────────────────────────────────────┤
 * │ PROGRESS · bar · step · rate · estimate · [Cancel] · the log                             │
 * ├──────────────────────────────────────────────────────────────────────────────────────────┤
 * │ ● ffmpeg ready · segment counts · notice                                                 │
 * └──────────────────────────────────────────────────────────────────────────────────────────┘
 * ```
 *
 * The frame never scrolls. Two zones scroll inside themselves — the proof panel and the
 * log — because only those can hold an unbounded number of rows.
 *
 * ## Why the interface is arranged this way, in one paragraph
 *
 * `Trim` is the only button that starts work, and the row it sits in says what it will do
 * in the operator's own numbers rather than offering alternatives to it. Everything that
 * answered a different question moved to where that question is asked: the file picker is
 * the operating system's, Cancel is beside the bar it stops, the export is in the header of
 * what it exports. Colour is state and never decoration — one near-white primary control,
 * and four hues that each mean something.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { ProofPanel } from "./components/ProofPanel";
import { ProgressLog } from "./components/ProgressLog";
import {
  ApiFailure,
  api,
  desktop,
  listen,
  openVideo,
  reveal,
  type EngineEvent,
  type Health,
  type MediaView,
  type OutcomeView,
  type PlanView,
} from "./api";
import { useRunLog } from "./state/useRunLog";

/** CRF and preset are the encoder's, so they are named after it rather than invented. */
const QUALITY = [
  { crf: 18, label: "18 — visually lossless (default)" },
  { crf: 20, label: "20 — a little smaller, still clean" },
  { crf: 23, label: "23 — smaller, and you can tell" },
  { crf: 28, label: "28 — small; for review copies" },
] as const;

const PRESETS = ["ultrafast", "veryfast", "fast", "medium", "slow"] as const;

export function App(): JSX.Element {
  const log = useRunLog();

  const [health, setHealth] = useState<Health | null>(null);
  const [text, setText] = useState(""); // what is in the path field, before it is loaded
  const [media, setMedia] = useState<MediaView | null>(null);
  const [transcript, setTranscript] = useState<string | null>(null);
  const [summary, setSummary] = useState("");
  const [inText, setInText] = useState("");
  const [outText, setOutText] = useState("");
  const [inFrame, setInFrame] = useState<number | null>(null);
  const [endFrame, setEndFrame] = useState<number | null>(null);
  const [crf, setCrf] = useState(18);
  const [preset, setPreset] = useState("veryfast");
  const [verify, setVerify] = useState("standard");
  // The segment's own name, and whether it arrives in a folder of that name. Both are empty by
  // default, which leaves the output called after its range exactly as it always was.
  const [segmentName, setSegmentName] = useState("");
  const [inFolder, setInFolder] = useState(false);
  // Where the plan says the segment will go. Shown rather than described, because the name and
  // the folder decide it and the only way to be sure of a path is to read it back.
  const [plannedOutput, setPlannedOutput] = useState("");
  const [plan, setPlan] = useState<PlanView | null>(null);
  const [plannedFor, setPlannedFor] = useState<string>("");
  const [outcome, setOutcome] = useState<OutcomeView | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [fullscreen, setFullscreen] = useState(false);

  const running = log.running;
  const rate = media === null ? 25 : media.rate.numerator / media.rate.denominator;
  const rateNumber = media === null ? 25 : Math.round(rate);

  // ---- startup: what can this machine do ------------------------------------------------
  useEffect(() => {
    void api.health().then(setHealth).catch((error: unknown) => setFailure(String(error)));
  }, []);

  useEffect(() => {
    void window.electronAPI?.isFullscreen?.().then((value) => setFullscreen(value === true));
  }, []);

  // ---- the run's own events --------------------------------------------------------------
  const onEvent = useCallback(
    (event: EngineEvent) => {
      switch (event.type) {
        case "started":
          // The run has begun *here*, not when the button was pressed: the engine is the one
          // that decides a run exists, and the clock and the bar start when it says so.
          log.begin();
          setOutcome(null);
          setFailure(null);
          break;
        case "progress":
          log.tick(event);
          break;
        case "log":
          log.line(event.text, event.level);
          break;
        case "command":
          log.command(event.args);
          break;
        case "stage":
          log.stage(event.label);
          break;
        case "finished": {
          setOutcome(event.outcome);
          const { outcome } = event;
          log.finish(
            event.ok,
            event.ok
              ? `done: ${outcome.frames.toLocaleString()} frames written in ${outcome.duration.toFixed(3)}s`
              : `written, but a check failed — see the proof panel`,
          );
          break;
        }
        case "failed":
          setFailure(event.message);
          log.finish(false, event.message);
          break;
        case "cancelled":
          setFailure(null);
          log.cancelled();
          break;
        default:
          break;
      }
    },
    [log],
  );

  /**
   * Subscribe to the run exactly once.
   *
   * `onEvent` is rebuilt whenever the run's state changes, which during a cut is several times
   * a second. Depending on it closed and reopened the stream on every one of those changes —
   * and because the backend replays its recent history to a new subscriber, each reconnection
   * delivered progress readings, which changed the state, which reconnected again. A feedback
   * loop, and the backend's log was a wall of connection resets.
   *
   * So the subscription is created once and the handler is reached through a ref, which is
   * always the newest one. The stream's lifetime is not the handler's lifetime.
   */
  const handler = useRef(onEvent);
  useEffect(() => {
    handler.current = onEvent;
  }, [onEvent]);

  useEffect(() => listen((event) => handler.current(event)), []);

  // ---- loading a file ---------------------------------------------------------------------
  const load = useCallback(async (candidate: string) => {
    const wanted = candidate.trim();
    if (wanted === "") {
      return;
    }
    setBusy("reading the file");
    setFailure(null);
    try {
      const answer = await api.probe(wanted);
      setMedia(answer.media);
      setText(answer.media.path);
      setTranscript(answer.transcript);
      setSummary(answer.summary);
      setInText("");
      setOutText("");
      setInFrame(null);
      setEndFrame(null);
      setPlan(null);
      setOutcome(null);
    } catch (caught) {
      setFailure(caught instanceof ApiFailure ? caught.message : String(caught));
    } finally {
      setBusy(null);
    }
  }, []);

  const browse = useCallback(async () => {
    const chosen = await openVideo();
    if (chosen !== null) {
      await load(chosen);
    }
  }, [load]);

  // ---- the marks --------------------------------------------------------------------------
  /**
   * Parse a timecode through the engine rather than in JavaScript.
   *
   * Drop-frame arithmetic is where a second implementation silently disagrees by a frame,
   * and this product's whole claim is that it does not.
   */
  const parseInto = useCallback(
    async (which: "in" | "out") => {
      const source = media?.path;
      const value = which === "in" ? inText : outText;
      if (source === undefined || value.trim() === "") {
        if (which === "in") {
          setInFrame(null);
        } else {
          setEndFrame(null);
        }
        return;
      }
      try {
        const parsed = await api.parse(source, value);
        if (which === "in") {
          setInFrame(parsed.frame);
        } else {
          setEndFrame(parsed.frame + 1);
        }
        setFailure(null);
      } catch (caught) {
        if (which === "in") {
          setInFrame(null);
        } else {
          setEndFrame(null);
        }
        setFailure(caught instanceof ApiFailure ? caught.message : String(caught));
      }
    },
    [inText, media?.path, outText],
  );

  useEffect(() => {
    void parseInto("in");
  }, [parseInto, inText]);

  useEffect(() => {
    void parseInto("out");
  }, [parseInto, outText]);

  const frames = inFrame !== null && endFrame !== null ? Math.max(0, endFrame - inFrame) : null;
  const ready = frames !== null && frames > 0;

  /**
   * The marks a plan would have to describe to still be current.
   *
   * The plan arrives a quarter of a second after the marks settle, so for that quarter second
   * there is an answer on screen to a question that is no longer being asked. Comparing the
   * signature is what stops a stale plan being shown as though it described the range in the
   * fields — and what stops `Trim` being pressed against one.
   */
  const marks =
    inFrame !== null && endFrame !== null
      ? `${inFrame}-${endFrame}-${crf}-${preset}-${segmentName.trim()}-${inFolder}`
      : "";
  const planned = plan !== null && marks !== "" && plannedFor === marks;

  // ---- the plan, as the marks are typed ----------------------------------------------------
  const planRef = useRef(0);
  useEffect(() => {
    if (!ready || media === null || inFrame === null || endFrame === null) {
      setPlan(null);
      return;
    }
    const ticket = planRef.current + 1;
    planRef.current = ticket;
    const timer = window.setTimeout(() => {
      void api
        .plan({
          source: media.path,
          inFrame,
          endFrame,
          rate,
          crf,
          preset,
          name: segmentName.trim(),
          inFolder,
        })
        .then((answer) => {
          // Only the newest answer is kept: a stale plan describing marks that have moved on
          // is worse than no plan, because it looks current.
          if (planRef.current === ticket) {
            setPlan(answer.plan);
            setPlannedOutput(answer.output);
            setPlannedFor(`${inFrame}-${endFrame}-${crf}-${preset}-${segmentName.trim()}-${inFolder}`);
          }
        })
        .catch(() => {
          if (planRef.current === ticket) {
            setPlan(null);
            // A name the filesystem would refuse is refused by the engine, and the sentence it
            // sends back belongs next to the field that was typed into. The path is cleared so
            // nothing stale is shown as though it were the destination.
            setPlannedOutput("");
          }
        });
    }, 250);
    return () => window.clearTimeout(timer);
  }, [crf, endFrame, inFrame, inFolder, media, preset, rate, ready, segmentName]);

  // ---- the cut ------------------------------------------------------------------------------
  const trim = useCallback(async () => {
    if (!ready || media === null || inFrame === null || endFrame === null) {
      return;
    }
    setBusy("starting");
    setFailure(null);
    try {
      await api.cut({
        source: media.path,
        inFrame,
        endFrame,
        rate,
        crf,
        preset,
        verify,
        name: segmentName.trim(),
        inFolder,
      });
    } catch (caught) {
      setFailure(caught instanceof ApiFailure ? caught.message : String(caught));
    } finally {
      setBusy(null);
    }
  }, [crf, endFrame, inFrame, inFolder, media, preset, rate, ready, segmentName, verify]);

  const toggleFullscreen = useCallback(async () => {
    const value = await window.electronAPI?.toggleFullscreen?.();
    setFullscreen(value === true);
  }, []);

  // ---- shortcuts ----------------------------------------------------------------------------
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "F11") {
        event.preventDefault();
        void toggleFullscreen();
        return;
      }
      if (!event.ctrlKey && !event.metaKey) {
        return;
      }
      switch (event.key.toLowerCase()) {
        case "enter":
          event.preventDefault();
          void trim();
          break;
        case "o":
        case "m":
          event.preventDefault();
          void browse();
          break;
        default:
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [browse, toggleFullscreen, trim]);

  const planText = useMemo(() => {
    // A plan describes one range. Marks that have moved on since it was asked for have no
    // plan yet, and showing the last one would describe a cut nobody is about to make.
    if (!ready || !planned || plan === null) {
      return null;
    }
    if (plan.mode === "copy") {
      return { tone: "ok", text: "lossless copy — every frame is the original" };
    }
    if (plan.mode === "headpatch") {
      return {
        tone: "warn",
        text: `head patch — ${plan.headFrames} frame(s) re-encoded, ${plan.bodyFrames} copied`,
      };
    }
    return { tone: "danger", text: "full re-encode — no keyframe inside this range" };
  }, [plan, planned, ready]);

  const inNumber = inFrame === null ? "" : inFrame.toLocaleString();
  const outNumber = endFrame === null ? "" : (endFrame - 1).toLocaleString();

  return (
    <div className="app">
      <header className="titlebar">
        <span className="titlebar__mark" aria-hidden="true">
          T
        </span>
        <h1 className="titlebar__product">TheTrimmer</h1>
        <span className="titlebar__rule" aria-hidden="true" />
        <p className="titlebar__tagline">
          Frame-exact, lossless segment cutting &nbsp;·&nbsp; only the keyframe head is re-encoded
        </p>
        <span className="spacer" />
        <button
          type="button"
          className="btn btn--ghost btn--small"
          onClick={() => void toggleFullscreen()}
          title={fullscreen ? "Leave fullscreen (F11)" : "Hide the titlebar and the taskbar (F11)"}
        >
          {fullscreen ? "Restore" : "Fullscreen"}
        </button>
      </header>

      <div className="app__body">
        <div className="app__col app__col--form">
          {/* ---- VIDEO ------------------------------------------------------------------ */}
          <section className="zone">
            <header className="zone__head">
              <h2 className="zone__title">Video</h2>
              <span className="spacer" />
              <span className="zone__note">
                {media === null
                  ? "no file"
                  : `${media.width}×${media.height} · ${media.codec} · ${media.rateText} fps`}
              </span>
            </header>
            <div className="zone__body zone__body--tight">
              <div className="row">
                <input
                  id="video-path"
                  className="path-field"
                  type="text"
                  value={text}
                  placeholder="path to the video to trim"
                  spellCheck={false}
                  aria-label="Video file"
                  onChange={(event) => setText(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      void load(event.currentTarget.value);
                    }
                  }}
                />
                <button
                  type="button"
                  className="btn"
                  onClick={() => void browse()}
                  disabled={running}
                  title="Open the Windows file dialog (Ctrl+O)"
                >
                  Browse
                </button>
              </div>
              <p className="zone__note">
                {media === null
                  ? "Pick a video, or paste its path and press Enter."
                  : media.audio === null
                    ? `${media.frames.toLocaleString()} frames · ${media.pixFmt} · no audio`
                    : `${media.frames.toLocaleString()} frames · ${media.pixFmt} · ${media.audio.codec} ${media.audio.sampleRate} Hz ${media.audio.channels}ch`}
              </p>
            </div>
          </section>

          {/* ---- the caption file, which is found rather than chosen ---------------------- */}
          <section className="zone">
            <header className="zone__head">
              <h2 className="zone__title">Caption file</h2>
              <span className="spacer" />
              <span className="zone__note">
                {transcript === null ? "none found beside the video" : "retimed with the segment"}
              </span>
            </header>
            <div className="zone__body zone__body--tight">
              <input
                id="subtitle-path"
                className="path-field"
                type="text"
                value={transcript ?? ""}
                placeholder="no caption file beside this video"
                spellCheck={false}
                aria-label="Caption file"
                readOnly
              />
            </div>
          </section>

          {/* ---- RANGE ------------------------------------------------------------------- */}
          <section className="zone">
            <header className="zone__head">
              <h2 className="zone__title">Range</h2>
              <span className="spacer" />
              <span className="zone__note">Premiere timecode HH:MM:SS:FF</span>
            </header>
            <div className="zone__body">
              <div className="range">
                <div className="range__row">
                  <label className="field-row__label" htmlFor="trim-in">
                    In point
                  </label>
                  <input
                    id="trim-in"
                    className="timecode-field"
                    type="text"
                    inputMode="numeric"
                    autoComplete="off"
                    spellCheck={false}
                    value={inText}
                    placeholder="00:00:00:00"
                    aria-label="In point"
                    onChange={(event) => setInText(event.target.value)}
                  />
                  <span className="field-row__note">
                    {inFrame === null ? "first frame kept" : `frame ${inNumber}`}
                  </span>
                </div>
                <div className="range__row">
                  <label className="field-row__label" htmlFor="trim-out">
                    Out point
                  </label>
                  <input
                    id="trim-out"
                    className="timecode-field"
                    type="text"
                    inputMode="numeric"
                    autoComplete="off"
                    spellCheck={false}
                    value={outText}
                    placeholder="00:00:10:00"
                    aria-label="Out point"
                    onChange={(event) => setOutText(event.target.value)}
                  />
                  <span className="field-row__note">
                    {endFrame === null ? "last frame kept" : `frame ${outNumber}, the last one kept`}
                  </span>
                </div>
              </div>
              <p className="range__plan" aria-live="polite">
                {frames === null ? (
                  "Type both timecodes; the frame numbers appear here as you type."
                ) : (
                  <>
                    <span>
                      frames {inNumber} … {outNumber}
                    </span>
                    <span className="range__sep" aria-hidden="true" />
                    <span>{frames.toLocaleString()} frames</span>
                    {planText !== null ? (
                      <>
                        <span className="range__sep" aria-hidden="true" />
                        <span className={planText.tone}>{planText.text}</span>
                      </>
                    ) : null}
                  </>
                )}
              </p>
            </div>
          </section>

          {/* ---- OPTIONS ----------------------------------------------------------------- */}
          <section className="zone">
            <header className="zone__head">
              <h2 className="zone__title">Options</h2>
            </header>
            <div className="zone__body zone__body--tight">
              {/*
                One row, not two. `--frame-min-height` is derived from the option rows the form
                column holds, and an option row costs `--field-height` plus the body gap — 36 px.
                The first version of this put the name and the folder on separate rows and stacked
                the path under the input, ~96 px against the ~80 px of slack a maximised 1080p
                window has: the column overflowed into its scroll valve and pushed these controls
                below the fold, where they looked like they had not been built at all.
              */}
              <div className="field-row">
                <label className="field-row__label" htmlFor="trim-name">
                  Name
                </label>
                <div className="field-row__value field-row__value--name">
                  <input
                    id="trim-name"
                    type="text"
                    value={segmentName}
                    placeholder="named for its range"
                    spellCheck={false}
                    autoComplete="off"
                    title={
                      segmentName.trim() === ""
                        ? "Leave empty to name the segment for its range, as before."
                        : plannedOutput
                    }
                    onChange={(event) => setSegmentName(event.target.value)}
                  />
                  <label
                    className="field-row__toggle"
                    title="Put the segment and its transcript in a folder of this name"
                  >
                    <input
                      id="trim-folder"
                      type="checkbox"
                      checked={inFolder}
                      disabled={segmentName.trim() === ""}
                      onChange={(event) => setInFolder(event.target.checked)}
                    />
                    <span>folder</span>
                  </label>
                  {/* The path is read back from the engine rather than rebuilt here, so what is
                      shown is what will be written -- including a name the engine refuses, which
                      is why this line says so rather than lying when the plan fails. It ellipsises
                      from the left, because the file's own name is the part worth reading. */}
                  <span className="field-row__note" title={plannedOutput || undefined}>
                    {segmentName.trim() === ""
                      ? "leave empty for the range name"
                      : plannedOutput === ""
                        ? "that name cannot be used — see the reason below"
                        : plannedOutput}
                  </span>
                </div>
              </div>
              <div className="field-row">
                <label className="field-row__label" htmlFor="trim-quality">
                  Quality
                </label>
                <div className="field-row__value">
                  <select
                    id="trim-quality"
                    value={crf}
                    onChange={(event) => setCrf(Number(event.target.value))}
                  >
                    {QUALITY.map((entry) => (
                      <option key={entry.crf} value={entry.crf}>
                        {entry.label}
                      </option>
                    ))}
                  </select>
                </div>
              </div>
              <div className="field-row">
                <label className="field-row__label" htmlFor="trim-preset">
                  Speed
                </label>
                <div className="field-row__value">
                  <select
                    id="trim-preset"
                    value={preset}
                    onChange={(event) => setPreset(event.target.value)}
                  >
                    {PRESETS.map((name) => (
                      <option key={name} value={name}>
                        {name}
                      </option>
                    ))}
                  </select>
                  <span className="field-row__note">applies to the head only</span>
                </div>
              </div>
              <div className="field-row">
                <label className="field-row__label" htmlFor="trim-verify">
                  Verify
                </label>
                <div className="field-row__value">
                  <select
                    id="trim-verify"
                    value={verify}
                    onChange={(event) => setVerify(event.target.value)}
                  >
                    <option value="off">Off — do not measure the result</option>
                    <option value="standard">
                      Standard — structure, length, and frame-for-frame alignment
                    </option>
                  </select>
                </div>
              </div>
            </div>
          </section>

          {/* ---- the one button ------------------------------------------------------------ */}
          <div className="actions">
            <p className="actions__state" aria-live="polite">
              {running ? (
                "cutting…"
              ) : failure !== null ? (
                <span className="danger">{failure}</span>
              ) : !ready ? (
                "No range marked. Type an in point and an out point."
              ) : (
                <>
                  <span className="figures">{frames?.toLocaleString()} frames</span>
                  <span className="range__sep" aria-hidden="true" />
                  {planText !== null ? (
                    <span className={planText.tone}>{planText.text}</span>
                  ) : (
                    // The button is disabled for this quarter second, and a disabled button
                    // with no reason beside it is a button that looks broken.
                    <span>reading the range…</span>
                  )}
                </>
              )}
            </p>
            <span className="spacer" />
            <button
              type="button"
              className="btn btn--primary btn--big"
              onClick={() => void trim()}
              disabled={running || !ready || !planned || busy !== null}
              title="Cut the marked range, then measure the finished file against the source (Ctrl+Enter)"
            >
              {running ? "Working…" : "Trim"}
            </button>
          </div>
        </div>

        {/* ---- what came out ------------------------------------------------------------- */}
        <div className="app__col app__col--results">
          <section className="zone">
            <header className="zone__head">
              <h2 className="zone__title">What came out</h2>
              <span className="spacer" />
              <span className="zone__note">
                {outcome === null
                  ? "nothing has run yet"
                  : `${outcome.frames.toLocaleString()} frames · ${outcome.duration.toFixed(3)}s`}
              </span>
              {/*
                There is no export here, and there is no button pretending to be one. What the
                panel holds is the finished file, and the useful verb on a finished file is
                "show me where it is" — which the window can actually do.
              */}
              <button
                type="button"
                className="btn btn--ghost btn--small"
                disabled={outcome === null || running || desktop.reveal === undefined}
                title={
                  desktop.reveal === undefined
                    ? "Showing a file in Explorer needs the desktop window"
                    : "Show the finished file in Explorer"
                }
                onClick={() => {
                  if (outcome !== null) {
                    void reveal(outcome.output).catch((error: unknown) =>
                      setFailure(error instanceof ApiFailure ? error.message : String(error)),
                    );
                  }
                }}
              >
                Show in Explorer
              </button>
            </header>
            <div className="zone__body zone__body--flush">
              <ProofPanel outcome={outcome} />
            </div>
          </section>
        </div>
      </div>

      {/* ---- progress -------------------------------------------------------------------- */}
      <section className="zone">
        <header className="zone__head">
          <h2 className="zone__title">Progress</h2>
          <span className="spacer" />
          {log.lines.length > 0 ? (
            <button type="button" className="btn btn--ghost btn--small" onClick={log.clear}>
              Clear
            </button>
          ) : null}
        </header>
        <ProgressLog
          lines={log.lines}
          progress={log.progress}
          running={running}
          elapsed={log.elapsed}
          onCancel={() => void api.cancel()}
        />
      </section>

      {/* ---- footer ---------------------------------------------------------------------- */}
      <footer className="footerline">
        <span className="footerline__item">
          <span
            className={`status ${
              health === null ? "status--idle" : health.libx264 ? "status--ok" : "status--danger"
            }`}
            title={health?.ffmpeg ?? "checking ffmpeg"}
          >
            {health === null ? "ffmpeg" : health.libx264 ? "ffmpeg ready" : "no H.264"}
          </span>
          {health !== null ? <span className="footerline__detail">{health.ffmpeg}</span> : null}
        </span>
        <span className="footerline__sep" aria-hidden="true" />
        <span className="footerline__item figures">
          {media === null ? "no file loaded" : `${media.durationText} long`}
        </span>
        <span className="spacer" />
        {summary !== "" ? <span className="footerline__notice">{summary}</span> : null}
      </footer>

      {/* The path field's own label, for the tests and for a screen reader. */}
      <span className="sr-only" aria-hidden="false">
        TheTrimmer {health?.version ?? ""}
      </span>
    </div>
  );
}

/** The bridge Electron installs. Declared here so the window type-checks without it. */
declare global {
  interface Window {
    readonly electronAPI?: {
      openVideo?(): Promise<string | null>;
      reveal?(path: string): Promise<void>;
      toggleFullscreen?(): Promise<boolean>;
      isFullscreen?(): Promise<boolean>;
      backendUrl?(): Promise<string>;
    };
  }
}
