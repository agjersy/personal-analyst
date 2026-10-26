import { create } from "zustand";
import type { AppInfo, KernelState, ModelEntry, SessionId, SessionInfo, SessionSummary, TargetInfo } from "../ipc";

export interface KernelStatus {
  state: KernelState;
  message: string | null;
}

interface SessionsState {
  app: AppInfo | null;
  sessions: SessionSummary[];
  infos: Record<SessionId, SessionInfo>;
  kernel: Record<SessionId, KernelStatus>;
  active: SessionId | null;
  models: ModelEntry[];
  targets: TargetInfo[];
  setApp(app: AppInfo): void;
  setSessions(list: SessionSummary[]): void;
  setInfo(info: SessionInfo): void;
  setKernel(session: SessionId, state: KernelState, message: string | null): void;
  setActive(session: SessionId | null): void;
  setModels(models: ModelEntry[]): void;
  setTargets(targets: TargetInfo[]): void;
  markClosed(session: SessionId): void;
}

export const useSessions = create<SessionsState>((set) => ({
  app: null,
  sessions: [],
  infos: {},
  kernel: {},
  active: null,
  models: [],
  targets: [],
  setApp: (app) => set({ app }),
  setSessions: (sessions) => set({ sessions }),
  setInfo: (info) =>
    set((s) => {
      const { policy: _p, conversations: _c, ...summary } = info;
      void _p;
      void _c;
      const exists = s.sessions.some((x) => x.id === info.id);
      const sessions = exists
        ? s.sessions.map((x) => (x.id === info.id ? { ...x, ...summaryOf(summary) } : x))
        : [summaryOf(summary), ...s.sessions];
      return {
        infos: { ...s.infos, [info.id]: info },
        sessions,
        kernel: s.kernel[info.id] ? s.kernel : { ...s.kernel, [info.id]: { state: info.kernel, message: null } },
      };
    }),
  setKernel: (session, state, message) => set((s) => ({ kernel: { ...s.kernel, [session]: { state, message } } })),
  setActive: (active) => set({ active }),
  setModels: (models) => set({ models }),
  setTargets: (targets) => set({ targets }),
  markClosed: (session) =>
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === session ? { ...x, open: false } : x)),
      kernel: { ...s.kernel, [session]: { state: "stopped", message: null } },
      active: s.active === session ? (s.sessions.find((x) => x.open && x.id !== session)?.id ?? null) : s.active,
    })),
}));

function summaryOf(x: SessionSummary & Partial<SessionInfo>): SessionSummary {
  return {
    id: x.id,
    name: x.name,
    repo_abs: x.repo_abs,
    target: x.target,
    model: x.model,
    created_at: x.created_at,
    last_opened_at: x.last_opened_at,
    open: x.open,
  };
}

export function useActiveSession(): SessionId | null {
  return useSessions((s) => s.active);
}

export function useSessionInfo(session: SessionId | null): SessionInfo | null {
  return useSessions((s) => (session ? (s.infos[session] ?? null) : null));
}

export function useKernel(session: SessionId | null): KernelStatus {
  return useSessions((s) => (session ? (s.kernel[session] ?? OFFLINE) : OFFLINE));
}

const OFFLINE: KernelStatus = { state: "stopped", message: null };
