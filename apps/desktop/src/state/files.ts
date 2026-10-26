import { create } from "zustand";
import type { FileEntry, IndexResult, SessionId } from "../ipc";

export interface EditorBuffer {
  path: string;
  /** Current editor text. */
  content: string;
  /** Text as last loaded from or written to disk. */
  saved: string;
  /** sha256 of `saved` as reported by the backend. */
  hash: string;
  dirty: boolean;
  /** Set when `files_changed` touched this path while the buffer is clean or dirty. */
  external: "none" | "changed" | "deleted";
  loading: boolean;
  error: string | null;
  /** Bumped when the store replaces the text so the editor reloads it. */
  revision: number;
}

export interface RevealRequest {
  path: string;
  line: number | null;
  nonce: number;
}

interface FilesState {
  /** Directory children keyed by directory path ("" = repo root). */
  trees: Record<SessionId, Record<string, FileEntry[]>>;
  expanded: Record<SessionId, Record<string, boolean>>;
  buffers: Record<SessionId, Record<string, EditorBuffer>>;
  index: Record<SessionId, IndexResult | null>;
  indexStale: Record<SessionId, boolean>;
  activeFile: Record<SessionId, string | null>;
  reveal: Record<SessionId, RevealRequest | null>;
  setTree(session: SessionId, dir: string, entries: FileEntry[]): void;
  setExpanded(session: SessionId, dir: string, expanded: boolean): void;
  invalidateDirs(session: SessionId, paths: string[]): void;
  setBuffer(session: SessionId, buffer: EditorBuffer): void;
  loadingBuffer(session: SessionId, path: string): void;
  bufferError(session: SessionId, path: string, error: string): void;
  updateContent(session: SessionId, path: string, content: string): void;
  markSaved(session: SessionId, path: string, hash: string): void;
  markExternal(session: SessionId, paths: string[], kind: "changed" | "deleted"): void;
  clearExternal(session: SessionId, path: string): void;
  closeBuffer(session: SessionId, path: string): void;
  renameBuffer(session: SessionId, from: string, to: string): void;
  setIndex(session: SessionId, index: IndexResult): void;
  markIndexStale(session: SessionId): void;
  setActiveFile(session: SessionId, path: string | null): void;
  requestReveal(session: SessionId, path: string, line: number | null): void;
}

export function dirOf(path: string): string {
  const i = path.lastIndexOf("/");
  return i < 0 ? "" : path.slice(0, i);
}

export const useFiles = create<FilesState>((set) => ({
  trees: {},
  expanded: {},
  buffers: {},
  index: {},
  indexStale: {},
  activeFile: {},
  reveal: {},
  setTree: (session, dir, entries) =>
    set((s) => ({ trees: { ...s.trees, [session]: { ...(s.trees[session] ?? {}), [dir]: entries } } })),
  setExpanded: (session, dir, expanded) =>
    set((s) => ({ expanded: { ...s.expanded, [session]: { ...(s.expanded[session] ?? {}), [dir]: expanded } } })),
  invalidateDirs: (session, paths) =>
    set((s) => {
      const tree = { ...(s.trees[session] ?? {}) };
      for (const p of paths) {
        delete tree[dirOf(p)];
        delete tree[p];
      }
      return { trees: { ...s.trees, [session]: tree } };
    }),
  setBuffer: (session, buffer) =>
    set((s) => ({ buffers: { ...s.buffers, [session]: { ...(s.buffers[session] ?? {}), [buffer.path]: buffer } } })),
  loadingBuffer: (session, path) =>
    set((s) => {
      const existing = s.buffers[session]?.[path];
      const buffer: EditorBuffer = existing
        ? { ...existing, loading: true }
        : { path, content: "", saved: "", hash: "", dirty: false, external: "none", loading: true, error: null, revision: 0 };
      return { buffers: { ...s.buffers, [session]: { ...(s.buffers[session] ?? {}), [path]: buffer } } };
    }),
  bufferError: (session, path, error) =>
    set((s) => {
      const existing = s.buffers[session]?.[path];
      if (!existing) return {};
      return { buffers: { ...s.buffers, [session]: { ...s.buffers[session], [path]: { ...existing, loading: false, error } } } };
    }),
  updateContent: (session, path, content) =>
    set((s) => {
      const existing = s.buffers[session]?.[path];
      if (!existing || existing.content === content) return {};
      return {
        buffers: { ...s.buffers, [session]: { ...s.buffers[session], [path]: { ...existing, content, dirty: content !== existing.saved } } },
      };
    }),
  markSaved: (session, path, hash) =>
    set((s) => {
      const existing = s.buffers[session]?.[path];
      if (!existing) return {};
      return {
        buffers: {
          ...s.buffers,
          [session]: { ...s.buffers[session], [path]: { ...existing, saved: existing.content, hash, dirty: false, external: "none" } },
        },
      };
    }),
  markExternal: (session, paths, kind) =>
    set((s) => {
      const bufs = s.buffers[session];
      if (!bufs) return {};
      let changed = false;
      const next = { ...bufs };
      for (const p of paths) {
        const b = next[p];
        if (b) {
          next[p] = { ...b, external: kind };
          changed = true;
        }
      }
      return changed ? { buffers: { ...s.buffers, [session]: next } } : {};
    }),
  clearExternal: (session, path) =>
    set((s) => {
      const b = s.buffers[session]?.[path];
      if (!b) return {};
      return { buffers: { ...s.buffers, [session]: { ...s.buffers[session], [path]: { ...b, external: "none" } } } };
    }),
  closeBuffer: (session, path) =>
    set((s) => {
      const bufs = { ...(s.buffers[session] ?? {}) };
      delete bufs[path];
      return { buffers: { ...s.buffers, [session]: bufs } };
    }),
  renameBuffer: (session, from, to) =>
    set((s) => {
      const bufs = { ...(s.buffers[session] ?? {}) };
      const b = bufs[from];
      if (!b) return {};
      delete bufs[from];
      bufs[to] = { ...b, path: to };
      return { buffers: { ...s.buffers, [session]: bufs } };
    }),
  setIndex: (session, index) =>
    set((s) => ({ index: { ...s.index, [session]: index }, indexStale: { ...s.indexStale, [session]: false } })),
  markIndexStale: (session) => set((s) => ({ indexStale: { ...s.indexStale, [session]: true } })),
  setActiveFile: (session, path) => set((s) => ({ activeFile: { ...s.activeFile, [session]: path } })),
  requestReveal: (session, path, line) =>
    set((s) => ({ reveal: { ...s.reveal, [session]: { path, line, nonce: (s.reveal[session]?.nonce ?? 0) + 1 } } })),
}));

export function useBuffer(session: SessionId | null, path: string | null): EditorBuffer | undefined {
  return useFiles((s) => (session && path ? s.buffers[session]?.[path] : undefined));
}

export function useIndex(session: SessionId | null): IndexResult | null {
  return useFiles((s) => (session ? (s.index[session] ?? null) : null));
}
