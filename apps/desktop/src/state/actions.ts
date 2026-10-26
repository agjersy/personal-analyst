// Async orchestration on top of the stores: every call goes through the Backend.
import { backend, errorMessage, getBackend, isIpcError } from "../ipc";
import type { Attachment, ConversationId, ProposalDecision, ProposalId, SessionId } from "../ipc";
import { useChat } from "./chat";
import { subscribeSession, unsubscribeSession } from "./dispatcher";
import { useFiles } from "./files";
import { useRuns } from "./runs";
import { useSessions } from "./sessions";
import { useTasks } from "./tasks";
import { dropTerminalBuffers, useTerminals } from "./terminals";
import { notifyError, useUi } from "./ui";

export async function bootstrap(): Promise<void> {
  const b = await getBackend();
  const sessions = useSessions.getState();
  const [app, models, targets, list] = await Promise.all([
    b.invoke("app_info", {}),
    b.invoke("models_list", {}),
    b.invoke("targets_list", {}),
    b.invoke("sessions_list", {}),
  ]);
  sessions.setApp(app);
  sessions.setModels(models);
  sessions.setTargets(targets);
  sessions.setSessions(list);
  const open = list.filter((s) => s.open);
  for (const s of open) await openSession(s.id);
  if (open[0]) sessions.setActive(open[0].id);
}

export async function openSession(session: SessionId): Promise<void> {
  const b = backend();
  await subscribeSession(session);
  const info = await b.invoke("session_open", { session });
  useSessions.getState().setInfo(info);
  useSessions.getState().setKernel(session, info.kernel, null);
  useSessions.getState().setActive(session);
  useChat.getState().ensure(info.active_conversation);
  await loadSessionData(session);
}

export async function createSession(args: { repo_abs: string; target?: string; model?: string; init?: boolean }): Promise<SessionId> {
  const info = await backend().invoke("session_create", args);
  await subscribeSession(info.id);
  const sessions = useSessions.getState();
  sessions.setInfo(info);
  sessions.setKernel(info.id, info.kernel, null);
  sessions.setActive(info.id);
  useChat.getState().ensure(info.active_conversation);
  await loadSessionData(info.id);
  return info.id;
}

export async function closeSession(session: SessionId): Promise<void> {
  await backend().invoke("session_close", { session });
  unsubscribeSession(session);
  for (const t of useTerminals.getState().terminals[session] ?? []) dropTerminalBuffers(t.id);
  useSessions.getState().markClosed(session);
}

export async function loadSessionData(session: SessionId): Promise<void> {
  const b = backend();
  const files = useFiles.getState();
  const runs = useRuns.getState();
  const [tree, index, runList, figures] = await Promise.all([
    b.invoke("fs_tree", { session, depth: 1 }),
    b.invoke("index_get", { session }),
    b.invoke("runs_list", { session, limit: 100 }),
    b.invoke("figures_list", { session }),
  ]);
  files.setTree(session, "", tree);
  files.setIndex(session, index);
  runs.setRuns(session, runList);
  runs.setFigures(session, figures);
}

export async function loadDir(session: SessionId, dir: string): Promise<void> {
  const entries = await backend().invoke("fs_tree", { session, path: dir, depth: 1 });
  useFiles.getState().setTree(session, dir, entries);
}

export async function refreshIndex(session: SessionId): Promise<void> {
  const index = await backend().invoke("index_get", { session });
  useFiles.getState().setIndex(session, index);
}

export async function loadBuffer(session: SessionId, path: string, force = false): Promise<void> {
  const files = useFiles.getState();
  const existing = files.buffers[session]?.[path];
  if (existing && !force && !existing.error) return;
  files.loadingBuffer(session, path);
  try {
    const fc = await backend().invoke("fs_read", { session, path });
    const cur = useFiles.getState().buffers[session]?.[path];
    files.setBuffer(session, {
      path,
      content: fc.content,
      saved: fc.content,
      hash: fc.hash,
      dirty: false,
      external: "none",
      loading: false,
      error: null,
      revision: (cur?.revision ?? 0) + 1,
    });
  } catch (e) {
    files.bufferError(session, path, errorMessage(e));
    throw e;
  }
}

export async function saveBuffer(session: SessionId, path: string, overwrite = false): Promise<boolean> {
  const buffer = useFiles.getState().buffers[session]?.[path];
  if (!buffer) return false;
  try {
    const res = await backend().invoke("fs_write", {
      session,
      path,
      content: buffer.content,
      ...(overwrite || !buffer.hash ? {} : { expected_hash: buffer.hash }),
    });
    useFiles.getState().markSaved(session, path, res.hash);
    return true;
  } catch (e) {
    if (isIpcError(e) && e.code === "conflict") {
      useFiles.getState().markExternal(session, [path], "changed");
      useUi.getState().toast("error", `${path} changed on disk; reload or overwrite from the banner.`);
      return false;
    }
    notifyError(e);
    return false;
  }
}

export async function runScript(session: SessionId, script: string, args: string[]): Promise<string | null> {
  try {
    const run = await backend().invoke("run_start", { session, script, args });
    useRuns.getState().upsertRun(session, run);
    useRuns.getState().selectRun(session, run.meta.run_id);
    return run.meta.run_id;
  } catch (e) {
    notifyError(e);
    return null;
  }
}

export async function loadRunOutput(session: SessionId, run_id: string): Promise<void> {
  const full = await backend().invoke("run_get", { session, run_id });
  useRuns.getState().setOutput(run_id, { stdout: full.stdout, stderr: full.stderr });
}

export async function ensureFigureData(session: SessionId, path: string): Promise<string> {
  const cached = useRuns.getState().figureData[path];
  if (cached) return cached;
  const data = await backend().invoke("figure_data", { session, path });
  const url = `data:${data.mime};base64,${data.base64}`;
  useRuns.getState().setFigureData(path, url);
  return url;
}

export async function sendChat(session: SessionId, conversation: ConversationId, text: string, attachments: Attachment[]): Promise<void> {
  const chat = useChat.getState();
  chat.addUser(conversation, text, attachments);
  chat.clearAttachments(conversation);
  chat.setDraft(conversation, "");
  try {
    await backend().invoke("chat_send", { session, conversation, text, ...(attachments.length ? { attachments } : {}) });
  } catch (e) {
    chat.setBusy(conversation, false);
    notifyError(e);
  }
}

export async function cancelChat(session: SessionId, conversation: ConversationId): Promise<void> {
  try {
    await backend().invoke("chat_cancel", { session, conversation });
  } catch (e) {
    notifyError(e);
  }
}

export async function respondProposal(session: SessionId, proposal_id: ProposalId, decision: ProposalDecision): Promise<void> {
  try {
    await backend().invoke("proposal_respond", { session, proposal_id, decision });
    useChat.getState().resolveProposal(proposal_id, decision.decision, "user");
  } catch (e) {
    notifyError(e);
  }
}

export async function newConversation(session: SessionId): Promise<void> {
  try {
    const conv = await backend().invoke("conversation_new", { session });
    useChat.getState().ensure(conv.id);
    const info = await backend().invoke("session_info", { session });
    useSessions.getState().setInfo(info);
  } catch (e) {
    notifyError(e);
  }
}

export async function selectConversation(session: SessionId, conversation: ConversationId): Promise<void> {
  try {
    await backend().invoke("conversation_select", { session, conversation });
    useChat.getState().ensure(conversation);
    const info = await backend().invoke("session_info", { session });
    useSessions.getState().setInfo(info);
  } catch (e) {
    notifyError(e);
  }
}

export async function openTerminal(session: SessionId, cols = 80, rows = 24): Promise<void> {
  try {
    const id = await backend().invoke("terminal_open", { session, cols, rows });
    const n = (useTerminals.getState().terminals[session]?.length ?? 0) + 1;
    useTerminals.getState().add(session, id, `Terminal ${n}`);
  } catch (e) {
    notifyError(e);
  }
}

export async function closeTerminal(session: SessionId, terminal_id: string): Promise<void> {
  useTerminals.getState().remove(session, terminal_id);
  dropTerminalBuffers(terminal_id);
  try {
    await backend().invoke("terminal_close", { terminal_id });
  } catch {
    // already gone
  }
}

export async function startTask(session: SessionId, label: string, fn: () => Promise<string>): Promise<void> {
  try {
    const id = await fn();
    useTasks.getState().start(session, id, label);
  } catch (e) {
    notifyError(e);
  }
}
