// Applies SessionEvents (IPC.md section 3) to the stores. One subscription per
// open session; events for sessions that are not active still update state.
import { backend } from "../ipc";
import type { SessionEvent, SessionId } from "../ipc";
import { useChat } from "./chat";
import { useDiagnostics } from "./diagnostics";
import { useFiles } from "./files";
import { checkExternalChanges } from "./fileWatch";
import { useRuns } from "./runs";
import { useSessions } from "./sessions";
import { useTasks } from "./tasks";
import { deliverTerminalData, dropTerminalBuffers, useTerminals } from "./terminals";
import { useUi } from "./ui";

export function applyEvent(session: SessionId, e: SessionEvent): void {
  switch (e.type) {
    case "agent_text":
      useChat.getState().appendText(e.conversation, e.turn, e.delta);
      break;
    case "agent_tool_call":
      useChat.getState().toolCall(e.conversation, e.turn, e.id, e.name, e.arguments);
      break;
    case "agent_tool_result":
      useChat.getState().toolResult(e.conversation, e.id, e.ok, e.summary);
      break;
    case "proposal":
      useChat.getState().addProposal(session, e.proposal);
      break;
    case "proposal_resolved":
      useChat.getState().resolveProposal(e.proposal_id, e.decision, e.by);
      break;
    case "turn_started":
      useChat.getState().turnStarted(e.conversation, e.turn);
      break;
    case "turn_done":
      useChat.getState().turnDone(e.conversation, e.turn, e.summary);
      break;
    case "turn_error":
      useChat.getState().turnError(e.conversation, e.turn, e.message);
      break;
    case "usage":
      useChat.getState().setUsage(e.conversation, {
        input_tokens: e.input_tokens,
        output_tokens: e.output_tokens,
        context_window: e.context_window,
        context_used: e.context_used,
      });
      break;
    case "run_started": {
      useRuns.getState().upsertRun(session, e.run);
      // Runs have no conversation id; a run that starts while a conversation
      // of this session is busy is shown as a card in that conversation.
      const info = useSessions.getState().infos[session];
      const chat = useChat.getState();
      const busy = info?.conversations.map((c) => c.id).find((id) => chat.conversations[id]?.busy);
      if (busy) chat.addRun(busy, e.run.meta.run_id);
      break;
    }
    case "run_output":
      useRuns.getState().appendOutput(e.run_id, e.stream, e.chunk);
      break;
    case "run_artifact":
      useRuns.getState().addArtifact(session, e.run_id, e.artifact, e.figure);
      break;
    case "run_done":
      useRuns.getState().upsertRun(session, e.run);
      break;
    case "diagnostics":
      useDiagnostics.getState().replace(session, e.paths, e.diagnostics);
      break;
    case "files_changed": {
      const files = useFiles.getState();
      files.invalidateDirs(session, e.paths);
      void checkExternalChanges(session, e.paths);
      if (e.paths.some((p) => p.endsWith(".py"))) files.markIndexStale(session);
      break;
    }
    case "index_changed":
      useFiles.getState().markIndexStale(session);
      break;
    case "terminal_data":
      deliverTerminalData(e.terminal_id, e.data);
      break;
    case "terminal_exit":
      useTerminals.getState().exited(e.terminal_id, e.code);
      dropTerminalBuffers(e.terminal_id);
      break;
    case "kernel_state":
      useSessions.getState().setKernel(session, e.state, e.message);
      if (e.state === "error" && e.message) useUi.getState().toast("error", `Kernel error: ${e.message}`);
      break;
    case "packages_changed":
      bumpPackages(session);
      break;
    case "task_log":
      useTasks.getState().log(e.task_id, e.line);
      break;
    case "task_done":
      useTasks.getState().done(e.task_id, e.ok, e.message);
      break;
    case "session_changed":
      useSessions.getState().setInfo(e.info);
      break;
  }
}

// Packages have no store of their own; the panel re-fetches on this counter.
const packageListeners = new Set<(session: SessionId) => void>();
export function onPackagesChanged(fn: (session: SessionId) => void): () => void {
  packageListeners.add(fn);
  return () => {
    packageListeners.delete(fn);
  };
}
function bumpPackages(session: SessionId): void {
  for (const fn of packageListeners) fn(session);
}

const subscriptions = new Map<SessionId, () => void>();

export async function subscribeSession(session: SessionId): Promise<void> {
  if (subscriptions.has(session)) return;
  subscriptions.set(session, () => undefined);
  const unsubscribe = await backend().subscribe(session, (e) => applyEvent(session, e));
  subscriptions.set(session, unsubscribe);
}

export function unsubscribeSession(session: SessionId): void {
  const un = subscriptions.get(session);
  if (un) {
    un();
    subscriptions.delete(session);
  }
}
