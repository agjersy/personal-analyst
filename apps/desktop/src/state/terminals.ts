import { create } from "zustand";
import type { SessionId, TerminalId } from "../ipc";

export interface TerminalTab {
  id: TerminalId;
  title: string;
  exit: number | null | undefined; // undefined = running
}

interface TerminalsState {
  terminals: Record<SessionId, TerminalTab[]>;
  active: Record<SessionId, TerminalId | null>;
  add(session: SessionId, id: TerminalId, title: string): void;
  remove(session: SessionId, id: TerminalId): void;
  setActive(session: SessionId, id: TerminalId | null): void;
  exited(id: TerminalId, code: number | null): void;
}

export const useTerminals = create<TerminalsState>((set) => ({
  terminals: {},
  active: {},
  add: (session, id, title) =>
    set((s) => ({
      terminals: { ...s.terminals, [session]: [...(s.terminals[session] ?? []), { id, title, exit: undefined }] },
      active: { ...s.active, [session]: id },
    })),
  remove: (session, id) =>
    set((s) => {
      const list = (s.terminals[session] ?? []).filter((t) => t.id !== id);
      const active = s.active[session] === id ? (list[list.length - 1]?.id ?? null) : (s.active[session] ?? null);
      return { terminals: { ...s.terminals, [session]: list }, active: { ...s.active, [session]: active } };
    }),
  setActive: (session, id) => set((s) => ({ active: { ...s.active, [session]: id } })),
  exited: (id, code) =>
    set((s) => {
      const terminals: Record<SessionId, TerminalTab[]> = {};
      for (const [session, list] of Object.entries(s.terminals)) {
        terminals[session] = list.map((t) => (t.id === id ? { ...t, exit: code } : t));
      }
      return { terminals };
    }),
}));

// Terminal byte streams bypass React state: xterm instances register a sink,
// and data that arrives before the sink exists is buffered.
const sinks = new Map<TerminalId, (data: string) => void>();
const buffered = new Map<TerminalId, string[]>();

export function attachTerminalSink(id: TerminalId, sink: (data: string) => void): () => void {
  sinks.set(id, sink);
  const pending = buffered.get(id);
  if (pending) {
    buffered.delete(id);
    for (const chunk of pending) sink(chunk);
  }
  return () => {
    if (sinks.get(id) === sink) sinks.delete(id);
  };
}

export function deliverTerminalData(id: TerminalId, data: string): void {
  const sink = sinks.get(id);
  if (sink) sink(data);
  else {
    const list = buffered.get(id) ?? [];
    list.push(data);
    buffered.set(id, list);
  }
}

export function dropTerminalBuffers(id: TerminalId): void {
  sinks.delete(id);
  buffered.delete(id);
}
