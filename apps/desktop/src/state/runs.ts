import { create } from "zustand";
import type { Artifact, FigureGroup, FigureView, RunId, RunView, SessionId } from "../ipc";

export interface RunOutput {
  stdout: string;
  stderr: string;
}

export interface FigureSelection {
  name: string;
  run_id: RunId;
}

interface RunsState {
  runs: Record<SessionId, RunView[]>;
  output: Record<RunId, RunOutput>;
  figures: Record<SessionId, FigureGroup[]>;
  selectedRun: Record<SessionId, RunId | null>;
  /** Figure currently shown in the Figures panel. */
  activeFigure: Record<SessionId, FigureSelection | null>;
  /** Figures pinned so newer runs do not replace them. */
  pinned: Record<SessionId, FigureSelection[]>;
  compare: Record<SessionId, RunId | null>;
  /** data: URLs by repo-relative figure path. */
  figureData: Record<string, string>;
  setRuns(session: SessionId, runs: RunView[]): void;
  upsertRun(session: SessionId, run: RunView): void;
  appendOutput(run_id: RunId, stream: "stdout" | "stderr", chunk: string): void;
  setOutput(run_id: RunId, output: RunOutput): void;
  addArtifact(session: SessionId, run_id: RunId, artifact: Artifact, figure: FigureView | null): void;
  setFigures(session: SessionId, groups: FigureGroup[]): void;
  selectRun(session: SessionId, run_id: RunId | null): void;
  setActiveFigure(session: SessionId, selection: FigureSelection | null): void;
  togglePin(session: SessionId, selection: FigureSelection): void;
  setCompare(session: SessionId, run_id: RunId | null): void;
  setFigureData(path: string, dataUrl: string): void;
}

export const useRuns = create<RunsState>((set) => ({
  runs: {},
  output: {},
  figures: {},
  selectedRun: {},
  activeFigure: {},
  pinned: {},
  compare: {},
  figureData: {},
  setRuns: (session, runs) => set((s) => ({ runs: { ...s.runs, [session]: runs } })),
  upsertRun: (session, run) =>
    set((s) => {
      const list = s.runs[session] ?? [];
      const idx = list.findIndex((r) => r.meta.run_id === run.meta.run_id);
      const existing = idx >= 0 ? list[idx] : undefined;
      const merged: RunView = existing
        ? { ...run, artifacts: [...run.artifacts, ...existing.artifacts.filter((a) => !run.artifacts.some((b) => b.path === a.path))] }
        : run;
      const next = idx >= 0 ? list.map((r, i) => (i === idx ? merged : r)) : [merged, ...list];
      const groups = s.figures[session] ?? [];
      const figures = groups.map((g) => (g.run.meta.run_id === run.meta.run_id ? { ...g, run: merged } : g));
      return { runs: { ...s.runs, [session]: next }, figures: { ...s.figures, [session]: figures } };
    }),
  appendOutput: (run_id, stream, chunk) =>
    set((s) => {
      const cur = s.output[run_id] ?? { stdout: "", stderr: "" };
      return { output: { ...s.output, [run_id]: { ...cur, [stream]: cur[stream] + chunk } } };
    }),
  setOutput: (run_id, output) => set((s) => ({ output: { ...s.output, [run_id]: output } })),
  addArtifact: (session, run_id, artifact, figure) =>
    set((s) => {
      const list = s.runs[session] ?? [];
      const runs = list.map((r) =>
        r.meta.run_id === run_id && !r.artifacts.some((a) => a.path === artifact.path) ? { ...r, artifacts: [...r.artifacts, artifact] } : r,
      );
      if (!figure) return { runs: { ...s.runs, [session]: runs } };
      const run = runs.find((r) => r.meta.run_id === run_id);
      const groups = s.figures[session] ?? [];
      const existing = groups.find((g) => g.run.meta.run_id === run_id);
      let figures: FigureGroup[];
      if (existing) {
        figures = groups.map((g) =>
          g.run.meta.run_id === run_id ? { ...g, figures: g.figures.some((f) => f.path === figure.path) ? g.figures : [...g.figures, figure] } : g,
        );
      } else if (run) {
        figures = [{ run, figures: [figure] }, ...groups];
      } else {
        figures = groups;
      }
      const pinned = s.pinned[session] ?? [];
      const active = s.activeFigure[session];
      const keepActive = active !== null && active !== undefined && pinned.some((p) => p.name === active.name && p.run_id === active.run_id);
      const activeFigure = keepActive ? active : { name: figure.name, run_id };
      return {
        runs: { ...s.runs, [session]: runs },
        figures: { ...s.figures, [session]: figures },
        activeFigure: { ...s.activeFigure, [session]: activeFigure },
      };
    }),
  setFigures: (session, groups) =>
    set((s) => {
      const first = groups[0]?.figures[0];
      const active = s.activeFigure[session] ?? (first ? { name: first.name, run_id: first.run_id } : null);
      return { figures: { ...s.figures, [session]: groups }, activeFigure: { ...s.activeFigure, [session]: active } };
    }),
  selectRun: (session, run_id) => set((s) => ({ selectedRun: { ...s.selectedRun, [session]: run_id } })),
  setActiveFigure: (session, selection) => set((s) => ({ activeFigure: { ...s.activeFigure, [session]: selection } })),
  togglePin: (session, selection) =>
    set((s) => {
      const list = s.pinned[session] ?? [];
      const has = list.some((p) => p.name === selection.name && p.run_id === selection.run_id);
      return {
        pinned: {
          ...s.pinned,
          [session]: has ? list.filter((p) => !(p.name === selection.name && p.run_id === selection.run_id)) : [...list, selection],
        },
      };
    }),
  setCompare: (session, run_id) => set((s) => ({ compare: { ...s.compare, [session]: run_id } })),
  setFigureData: (path, dataUrl) => set((s) => ({ figureData: { ...s.figureData, [path]: dataUrl } })),
}));

export function findRun(session: SessionId, run_id: RunId): RunView | undefined {
  return useRuns.getState().runs[session]?.find((r) => r.meta.run_id === run_id);
}
