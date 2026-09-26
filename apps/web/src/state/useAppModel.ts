/**
 * The application's state, in one hook.
 *
 * There is no state library here, and that is a decision rather than an omission. The state that
 * matters — the project, its sources, its segments, the run history — lives in Rust and changes only
 * through a command. What React holds is a *cache of the last answer*, which is refreshed by calling
 * the command again. A store that mirrored the project would be a second source of truth, and the two
 * would disagree exactly when it mattered: after a failed cut, or when a source went offline.
 *
 * So this hook is small on purpose: it holds the last known views, a busy flag, the last error, and
 * one `refresh` that re-asks. Every mutation calls `refresh` rather than patching the cache
 * optimistically, because a cut that the user believes succeeded and the application believes failed
 * is worse than a spinner.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { IpcFailure, commands } from "../ipc/commands";
import type {
  BatchOutcomeWire,
  DoctorReportWire,
  PresetWire,
  ProjectListingWire,
  QueuePreview,
  SegmentView,
  SourceView,
  WorkspaceSummary,
} from "../ipc/types";

/** What the interface needs to know, and nothing else. */
export interface AppModel {
  readonly ready: boolean;
  readonly busy: string | null;
  readonly error: IpcFailure | null;
  readonly notice: string | null;

  readonly projects: readonly ProjectListingWire[];
  readonly openProjectId: string | null;
  readonly sources: readonly SourceView[];
  readonly segments: readonly SegmentView[];
  readonly summary: WorkspaceSummary | null;
  readonly presets: readonly PresetWire[];
  readonly previews: readonly QueuePreview[];
  readonly lastOutcome: BatchOutcomeWire | null;
  readonly doctor: DoctorReportWire | null;

  /**
   * The session's name.
   *
   * There is one project and no picker, so this is not something the user chose — it is the name the
   * sessions list, the export metadata and the run log use, derived from the day the window was first
   * opened. `"TheTrimmer"` until one exists, which is only true for the first few hundred
   * milliseconds.
   */
  readonly sessionName: string;

  /** The verification policy for the whole project: `off`, `standard`, `strict` or `forensic`. */
  readonly verifyPolicy: string;
  readonly setVerifyPolicy: (value: string) => Promise<void>;

  readonly selectedSegment: string | null;
  readonly selectSegment: (id: string | null) => void;

  readonly clearMessages: () => void;
  readonly refresh: () => Promise<void>;
  readonly refreshProjects: () => Promise<void>;
  readonly openProject: (id: string) => Promise<void>;
  readonly createProject: (name: string) => Promise<void>;
  readonly deleteProject: (id: string) => Promise<void>;
  readonly addSource: (path: string) => Promise<void>;
  readonly removeSource: (path: string) => Promise<void>;
  readonly addSegment: (input: {
    readonly source: string;
    readonly name: string;
    readonly startFrame: number;
    readonly endFrame: number | null;
    readonly preset: string | null;
    readonly handleFrames: number;
  }) => Promise<void>;
  readonly updateSegment: (input: {
    readonly id: string;
    readonly name?: string;
    readonly startFrame?: number;
    readonly endFrame?: number | null;
    readonly preset?: string | null;
    readonly handleFrames?: number;
    readonly enabled?: boolean;
    readonly note?: string | null;
  }) => Promise<void>;
  readonly removeSegment: (id: string) => Promise<void>;
  readonly previewAll: (marked?: {
    readonly startFrame: number;
    readonly endFrame: number;
    readonly preset: string | null;
    readonly handleFrames: number;
  }) => Promise<void>;
  readonly planQuietly: (marked: {
    readonly startFrame: number;
    readonly endFrame: number;
    readonly preset: string | null;
    readonly handleFrames: number;
  }) => Promise<void>;
  readonly runBatch: (options: {
    readonly stopOnError: boolean;
    readonly skipVerification: boolean;
    readonly label: string;
  }) => Promise<void>;
  readonly cancelBatch: () => Promise<void>;
  readonly exportTimeline: (input: {
    readonly format: "premiere" | "fcpxml" | "edl" | "csv";
    readonly path: string;
    readonly sequenceName: string;
  }) => Promise<readonly string[] | null>;
  readonly reveal: (path: string) => Promise<void>;
}

/**
 * The hook.
 *
 * `withBusy` wraps every mutation so the busy label, the error and the refresh are handled once
 * rather than at twenty call sites — and so a mutation that forgets to refresh is impossible.
 */
export function useAppModel(): AppModel {
  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<IpcFailure | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [projects, setProjects] = useState<readonly ProjectListingWire[]>([]);
  const [openProjectId, setOpenProjectId] = useState<string | null>(null);
  const [sources, setSources] = useState<readonly SourceView[]>([]);
  const [segments, setSegments] = useState<readonly SegmentView[]>([]);
  const [summary, setSummary] = useState<WorkspaceSummary | null>(null);
  const [presets, setPresets] = useState<readonly PresetWire[]>([]);
  const [previews, setPreviews] = useState<readonly QueuePreview[]>([]);
  const [lastOutcome, setLastOutcome] = useState<BatchOutcomeWire | null>(null);
  const [doctor, setDoctor] = useState<DoctorReportWire | null>(null);
  const [selectedSegment, setSelectedSegment] = useState<string | null>(null);
  const [verifyPolicyState, setVerifyPolicyState] = useState("strict");
  const [sessionName, setSessionName] = useState("TheTrimmer");

  // A ref as well as the state, because `setVerifyPolicy` restores the previous value on a refusal
  // and reading it out of the closure would capture whatever it was when the callback was made.
  const policyRef = useRef(verifyPolicyState);
  policyRef.current = verifyPolicyState;

  // A ref rather than state: this is read inside callbacks that must not be re-created when it
  // changes, and re-creating them would re-run effects that call commands.
  const openRef = useRef<string | null>(null);

  const fail = useCallback((caught: unknown, what: string) => {
    const failure =
      caught instanceof IpcFailure ? caught : new IpcFailure(`${what} failed: ${String(caught)}`);
    setError(failure);
  }, []);

  const refreshProjects = useCallback(async () => {
    try {
      setProjects(await commands.listProjects());
    } catch (caught) {
      fail(caught, "listing projects");
    }
  }, [fail]);

  /** Re-ask for everything that can change after a mutation. */
  const refresh = useCallback(async () => {
    if (openRef.current === null) {
      setSources([]);
      setSegments([]);
      setSummary(null);
      setPreviews([]);
      return;
    }
    try {
      const [nextSources, nextSegments, nextSummary, nextPresets] = await Promise.all([
        commands.sources(),
        commands.segments(),
        commands.summary(),
        commands.presets(),
      ]);
      setSources(nextSources);
      setSegments(nextSegments);
      setSummary(nextSummary);
      setPresets(nextPresets);
    } catch (caught) {
      fail(caught, "reading the project");
    }
  }, [fail]);

  /** Run a mutation, then refresh. One place, so nothing can forget the second half. */
  const withBusy = useCallback(
    async <T,>(label: string, body: () => Promise<T>): Promise<T | null> => {
      setBusy(label);
      setError(null);
      try {
        const result = await body();
        await refresh();
        return result;
      } catch (caught) {
        fail(caught, label);
        return null;
      } finally {
        setBusy(null);
      }
    },
    [fail, refresh],
  );

  /**
   * Startup: read the environment, then open *the* project.
   *
   * There is no project picker and no "new project" step. The window keeps one project — the store
   * has exactly one row in it for a single-user desktop tool — and opens it on launch, so the first
   * thing on screen is the video and the two timecode fields rather than a dialog asking the user to
   * name something before they can do anything.
   *
   * `GET /v1/projects` returning nothing is the first run: a project is created with a name derived
   * from the date, because a name nobody chooses is better than a modal nobody wants. This is also
   * why the store is still here at all: a batch of nine segments that a restart forgets is worse
   * than the dialog it replaced.
   */
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const nextDoctor = await commands.doctor();
        if (cancelled) {
          return;
        }
        setDoctor(nextDoctor);

        const existing = await commands.listProjects();
        if (cancelled) {
          return;
        }
        const first = existing[0];
        if (first === undefined) {
          const name = defaultProjectName();
          const created = await commands.createProject(name, "desktop");
          if (cancelled) {
            return;
          }
          setProjects([{ id: created.id, name, updatedAt: nowSeconds() }]);
          setSessionName(name);
          openRef.current = created.id;
          setOpenProjectId(created.id);
        } else {
          setProjects(existing);
          setSessionName(first.name);
          await commands.openProject(first.id);
          if (cancelled) {
            return;
          }
          openRef.current = first.id;
          setOpenProjectId(first.id);
        }
        // The policy is a property of the project, so it is read after the project is open rather
        // than guessed at.
        const policy = await commands.getVerifyPolicy();
        if (cancelled) {
          return;
        }
        setVerifyPolicyState(policy);
        await refresh();
      } catch (caught) {
        if (!cancelled) {
          fail(caught, "starting up");
        }
      } finally {
        if (!cancelled) {
          setReady(true);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [fail, refresh]);

  /** The name for a project nobody asked to create: the day it was started. */
  function defaultProjectName(): string {
    return `Session ${new Date().toLocaleDateString(undefined, { month: "short", day: "numeric" })}`;
  }

  function nowSeconds(): number {
    return Math.floor(Date.now() / 1000);
  }

  const openProject = useCallback(
    async (id: string) => {
      setBusy("opening the project");
      setError(null);
      try {
        await commands.openProject(id);
        openRef.current = id;
        setOpenProjectId(id);
        setSelectedSegment(null);
        setLastOutcome(null);
        await refresh();
      } catch (caught) {
        fail(caught, "opening the project");
      } finally {
        setBusy(null);
      }
    },
    [fail, refresh],
  );

  const createProject = useCallback(
    async (name: string) => {
      setBusy("creating the project");
      setError(null);
      try {
        const created = await commands.createProject(name, "desktop");
        await refreshProjects();
        await openProject(created.id);
      } catch (caught) {
        fail(caught, "creating the project");
      } finally {
        setBusy(null);
      }
    },
    [fail, openProject, refreshProjects],
  );

  const deleteProject = useCallback(
    async (id: string) => {
      await withBusy("deleting the project", async () => {
        await commands.deleteProject(id);
        await refreshProjects();
        if (openRef.current === id) {
          openRef.current = null;
          setOpenProjectId(null);
        }
      });
    },
    [refreshProjects, withBusy],
  );

  /**
   * Change the verification policy.
   *
   * The value is applied optimistically and then confirmed, because the select is a control a person
   * moves and reads back — waiting for a round trip to move a dropdown makes it feel broken. If the
   * command refuses, the previous value is restored and the refusal lands in the status bar.
   */
  const setVerifyPolicy = useCallback(
    async (value: string) => {
      const previous = policyRef.current;
      setVerifyPolicyState(value);
      try {
        await commands.setVerifyPolicy(value);
      } catch (caught) {
        setVerifyPolicyState(previous);
        fail(caught, "setting the verification policy");
      }
    },
    [fail],
  );

  const addSource = useCallback(
    async (path: string) => {
      const result = await withBusy("probing the source", () => commands.addSource(path));
      if (result !== null) {
        setNotice(`${result.name}: ${result.summary}`);
      }
    },
    [withBusy],
  );

  const removeSource = useCallback(
    async (path: string) => {
      await withBusy("removing the source", () => commands.removeSource(path));
    },
    [withBusy],
  );

  const addSegment = useCallback<AppModel["addSegment"]>(
    async (input) => {
      const created = await withBusy("adding the segment", () => commands.addSegment(input));
      if (created !== null) {
        setSelectedSegment(created.id);
      }
    },
    [withBusy],
  );

  const updateSegment = useCallback<AppModel["updateSegment"]>(
    async (input) => {
      await withBusy("saving the segment", () => commands.updateSegment(input));
    },
    [withBusy],
  );

  const removeSegment = useCallback(
    async (id: string) => {
      await withBusy("removing the segment", () => commands.removeSegment(id));
      setSelectedSegment((current) => (current === id ? null : current));
    },
    [withBusy],
  );

  /**
   * Plan the batch, with the marked range included when there is one.
   *
   * The marked range is passed as well as the queue because the interface plans continuously while
   * the fields are being filled: a range that has been marked but not queued still needs an answer to
   * "will this re-encode", and it has no id yet to ask about on its own.
   */
  const previewAll = useCallback(
    async (marked?: {
      readonly startFrame: number;
      readonly endFrame: number;
      readonly preset: string | null;
      readonly handleFrames: number;
    }) => {
      const result = await withBusy("planning the batch", () => commands.previewAll(marked));
      if (result !== null) {
        setPreviews(result);
        const full = result.filter((item) => item.forcesFullEncode).length;
        setNotice(
          full === 0
            ? `${result.length} segment(s) planned, none needing a full re-encode`
            : `${result.length} segment(s) planned; ${full} will be fully re-encoded by their preset`,
        );
      }
    },
    [withBusy],
  );

  /**
   * Plan quietly, for the continuous planning under the marks.
   *
   * Deliberately not `previewAll`: that one raises the busy flag and writes a notice to the status
   * bar, which is right for a button press and wrong for something that runs on every pause in
   * typing — the status bar would spend the whole session saying "planning the batch".
   *
   * A failure is swallowed rather than reported. The only reason this call exists is to fill in a
   * sentence in the length line; if it cannot, the line says nothing about the mode, the marks are
   * still perfectly valid, and the trim itself does its own planning and reports its own refusal. A
   * red error appearing because a plan could not be *previewed* would be alarming and wrong.
   */
  const planQuietly = useCallback(
    async (marked: {
      readonly startFrame: number;
      readonly endFrame: number;
      readonly preset: string | null;
      readonly handleFrames: number;
    }) => {
      try {
        setPreviews(await commands.previewAll(marked));
      } catch {
        // See above: a preview that cannot be made is not an error the user needs to see.
      }
    },
    [],
  );

  const runBatch = useCallback<AppModel["runBatch"]>(
    async (options) => {
      setBusy("running the batch");
      setError(null);
      setNotice(null);
      try {
        const outcome = await commands.runBatch(options);
        setLastOutcome(outcome);
        await refresh();
        setNotice(
          outcome.cancelled
            ? "the batch was cancelled; the segments that ran are listed below"
            : `${outcome.jobs.length} segment(s): see the proof panel`,
        );
      } catch (caught) {
        fail(caught, "running the batch");
      } finally {
        setBusy(null);
      }
    },
    [fail, refresh],
  );

  const cancelBatch = useCallback(async () => {
    await withBusy("cancelling", () => commands.cancelBatch());
  }, [withBusy]);

  const exportTimeline = useCallback<AppModel["exportTimeline"]>(
    async (input) => {
      const result = await withBusy("exporting", () => commands.exportTimeline(input));
      if (result !== null) {
        setNotice(`wrote ${result.clips} clip(s)${result.warnings.length > 0 ? `; ${result.warnings.length} warning(s)` : ""}`);
        return result.warnings;
      }
      return null;
    },
    [withBusy],
  );

  const reveal = useCallback(
    async (path: string) => {
      try {
        await commands.reveal(path);
      } catch (caught) {
        fail(caught, "revealing the file");
      }
    },
    [fail],
  );

  const clearMessages = useCallback(() => {
    setError(null);
    setNotice(null);
  }, []);

  return useMemo(
    () => ({
      ready,
      busy,
      error,
      notice,
      projects,
      openProjectId,
      sources,
      segments,
      summary,
      presets,
      previews,
      lastOutcome,
      doctor,
      sessionName,
      verifyPolicy: verifyPolicyState,
      setVerifyPolicy,
      selectedSegment,
      selectSegment: setSelectedSegment,
      clearMessages,
      refresh,
      refreshProjects,
      openProject,
      createProject,
      deleteProject,
      addSource,
      removeSource,
      addSegment,
      updateSegment,
      removeSegment,
      previewAll,
      planQuietly,
      runBatch,
      cancelBatch,
      exportTimeline,
      reveal,
    }),
    [
      ready,
      busy,
      error,
      notice,
      projects,
      openProjectId,
      sources,
      segments,
      summary,
      presets,
      previews,
      lastOutcome,
      doctor,
      sessionName,
      verifyPolicyState,
      setVerifyPolicy,
      selectedSegment,
      clearMessages,
      refresh,
      refreshProjects,
      openProject,
      createProject,
      deleteProject,
      addSource,
      removeSource,
      addSegment,
      updateSegment,
      removeSegment,
      previewAll,
      planQuietly,
      runBatch,
      cancelBatch,
      exportTimeline,
      reveal,
    ],
  );
}
