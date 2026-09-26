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
  LicenceStatusWire,
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
  readonly licence: LicenceStatusWire | null;

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
  readonly preview: (id: string) => Promise<QueuePreview | null>;
  readonly previewAll: () => Promise<void>;
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
  const [licence, setLicence] = useState<LicenceStatusWire | null>(null);
  const [selectedSegment, setSelectedSegment] = useState<string | null>(null);

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

  // Startup: the doctor report and the licence are read once, because neither changes while the
  // window is open.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [nextDoctor, nextLicence, nextProjects] = await Promise.all([
          commands.doctor(),
          commands.licenceStatus(),
          commands.listProjects(),
        ]);
        if (cancelled) {
          return;
        }
        setDoctor(nextDoctor);
        setLicence(nextLicence);
        setProjects(nextProjects);
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
  }, [fail]);

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

  const preview = useCallback(
    async (id: string) => {
      return withBusy("planning", () => commands.preview(id));
    },
    [withBusy],
  );

  const previewAll = useCallback(async () => {
    const result = await withBusy("planning the batch", () => commands.previewAll());
    if (result !== null) {
      setPreviews(result);
      const full = result.filter((item) => item.forcesFullEncode).length;
      setNotice(
        full === 0
          ? `${result.length} segment(s) planned, none needing a full re-encode`
          : `${result.length} segment(s) planned; ${full} will be fully re-encoded by their preset`,
      );
    }
  }, [withBusy]);

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
      licence,
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
      preview,
      previewAll,
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
      licence,
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
      preview,
      previewAll,
      runBatch,
      cancelBatch,
      exportTimeline,
      reveal,
    ],
  );
}
