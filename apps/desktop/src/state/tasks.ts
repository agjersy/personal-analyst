import { create } from "zustand";
import type { SessionId, TaskId } from "../ipc";

export interface TaskRecord {
  id: TaskId;
  session: SessionId;
  label: string;
  log: string[];
  done: boolean;
  ok: boolean | null;
  message: string | null;
  started_at: number;
}

interface TasksState {
  tasks: Record<TaskId, TaskRecord>;
  start(session: SessionId, id: TaskId, label: string): void;
  log(id: TaskId, line: string): void;
  done(id: TaskId, ok: boolean, message: string | null): void;
}

export const useTasks = create<TasksState>((set) => ({
  tasks: {},
  start: (session, id, label) =>
    set((s) => ({
      tasks: {
        ...s.tasks,
        [id]: s.tasks[id] ? { ...s.tasks[id], label, session } : { id, session, label, log: [], done: false, ok: null, message: null, started_at: Date.now() },
      },
    })),
  log: (id, line) =>
    set((s) => {
      const t = s.tasks[id] ?? { id, session: "", label: id, log: [], done: false, ok: null, message: null, started_at: Date.now() };
      return { tasks: { ...s.tasks, [id]: { ...t, log: [...t.log, line] } } };
    }),
  done: (id, ok, message) =>
    set((s) => {
      const t = s.tasks[id] ?? { id, session: "", label: id, log: [], done: false, ok: null, message: null, started_at: Date.now() };
      return { tasks: { ...s.tasks, [id]: { ...t, done: true, ok, message } } };
    }),
}));

export function useSessionTasks(session: SessionId | null): TaskRecord[] {
  const tasks = useTasks((s) => s.tasks);
  if (!session) return [];
  return Object.values(tasks)
    .filter((t) => t.session === session)
    .sort((a, b) => b.started_at - a.started_at);
}
