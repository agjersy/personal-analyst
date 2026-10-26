import { create } from "zustand";
import type { Diagnostic, SessionId } from "../ipc";

interface DiagnosticsState {
  byPath: Record<SessionId, Record<string, Diagnostic[]>>;
  replace(session: SessionId, paths: string[], diagnostics: Diagnostic[]): void;
  clearPaths(session: SessionId, paths: string[]): void;
}

export const useDiagnostics = create<DiagnosticsState>((set) => ({
  byPath: {},
  replace: (session, paths, diagnostics) =>
    set((s) => {
      const next = { ...(s.byPath[session] ?? {}) };
      for (const p of paths) delete next[p];
      for (const d of diagnostics) (next[d.path] ??= []).push(d);
      return { byPath: { ...s.byPath, [session]: next } };
    }),
  clearPaths: (session, paths) =>
    set((s) => {
      const next = { ...(s.byPath[session] ?? {}) };
      for (const p of paths) delete next[p];
      return { byPath: { ...s.byPath, [session]: next } };
    }),
}));

const NONE: Diagnostic[] = [];

export function usePathDiagnostics(session: SessionId | null, path: string | null): Diagnostic[] {
  return useDiagnostics((s) => (session && path ? (s.byPath[session]?.[path] ?? NONE) : NONE));
}

export function useAllDiagnostics(session: SessionId | null): Diagnostic[] {
  const byPath = useDiagnostics((s) => (session ? s.byPath[session] : undefined));
  if (!byPath) return NONE;
  return Object.values(byPath).flat();
}
