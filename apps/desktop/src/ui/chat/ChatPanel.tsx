import { useEffect, useRef } from "react";
import { backend } from "../../ipc";
import type { Mode } from "../../ipc";
import { newConversation, selectConversation } from "../../state/actions";
import { useChat, useConversation } from "../../state/chat";
import { useActiveSession, useSessionInfo, useSessions } from "../../state/sessions";
import { notifyError } from "../../state/ui";
import { MessageText } from "./MessageText";
import { ChatInput } from "./ChatInput";
import { ErrorCard, ProposalCard, RunCard, SummaryCard, ToolCard } from "./cards";

const MODES: { value: Mode; label: string; hint: string }[] = [
  { value: "plan", label: "Plan", hint: "Read-only: the agent proposes a plan" },
  { value: "build", label: "Build", hint: "Edits are proposed for review" },
  { value: "run", label: "Run", hint: "Runs existing scripts; no edits" },
];

export function ChatPanel() {
  const session = useActiveSession();
  const info = useSessionInfo(session);
  const models = useSessions((s) => s.models);
  const conversationId = info?.active_conversation ?? null;
  const conv = useConversation(conversationId);
  const summary = info?.conversations.find((c) => c.id === conversationId) ?? null;
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (conversationId) useChat.getState().ensure(conversationId);
  }, [conversationId]);

  useEffect(() => {
    const el = listRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [conv.items]);

  if (!session || !info || !conversationId) return <div className="empty">No session</div>;

  const setMode = (mode: Mode) =>
    backend()
      .invoke("chat_set_mode", { session, conversation: conversationId, mode })
      .catch(notifyError);
  const setModel = (model: string) =>
    backend()
      .invoke("chat_set_model", { session, conversation: conversationId, model })
      .catch(notifyError);

  const usage = conv.usage;
  const pct = usage && usage.context_window > 0 ? Math.min(100, Math.round((usage.context_used / usage.context_window) * 100)) : 0;

  return (
    <div className="chat" data-testid="chat">
      <div className="panel-toolbar">
        <select
          aria-label="Conversation"
          value={conversationId}
          onChange={(e) => void selectConversation(session, e.target.value)}
          style={{ maxWidth: 160 }}
        >
          {info.conversations.map((c) => (
            <option key={c.id} value={c.id}>
              {c.title}
              {c.busy ? " …" : ""}
            </option>
          ))}
        </select>
        <button type="button" className="small" onClick={() => void newConversation(session)} aria-label="New conversation">
          +
        </button>
        <span className="grow" />
        <select aria-label="Mode" value={summary?.mode ?? "build"} onChange={(e) => void setMode(e.target.value as Mode)} data-testid="mode-select">
          {MODES.map((m) => (
            <option key={m.value} value={m.value} title={m.hint}>
              {m.label}
            </option>
          ))}
        </select>
        <select aria-label="Conversation model" value={summary?.model ?? info.model ?? ""} onChange={(e) => void setModel(e.target.value)} style={{ maxWidth: 150 }}>
          {models.map((m) => (
            <option key={`${m.provider}/${m.id}`} value={`${m.provider}/${m.id}`}>
              {m.name}
            </option>
          ))}
        </select>
      </div>
      <div className="chat-messages" ref={listRef} role="log" aria-live="polite">
        {conv.items.length === 0 ? (
          <div className="empty">
            {summary?.mode === "run" ? "Run mode: ask for sweeps and comparisons using existing scripts." : "Ask for an analysis. Edits are proposed as diffs for review."}
          </div>
        ) : null}
        {conv.items.map((item) => {
          switch (item.kind) {
            case "user":
              return (
                <div key={item.id} className="msg msg-user" data-testid="user-message">
                  {item.text}
                  {item.attachments.length ? <div className="small muted">{item.attachments.map((a) => a.path).join(", ")}</div> : null}
                </div>
              );
            case "assistant":
              return <MessageText key={item.id} text={item.text} streaming={item.streaming} />;
            case "tool":
              return <ToolCard key={item.id} item={item} />;
            case "proposal":
              return <ProposalCard key={item.id} proposalId={item.proposal_id} />;
            case "run":
              return <RunCard key={item.id} session={session} run_id={item.run_id} />;
            case "summary":
              return <SummaryCard key={item.id} summary={item.summary} />;
            case "error":
              return <ErrorCard key={item.id} message={item.message} />;
          }
        })}
      </div>
      <div className="row small muted" style={{ padding: "2px 10px" }} title={usage ? `${usage.context_used} of ${usage.context_window} context tokens` : "no usage yet"}>
        <div className="usage" aria-label="Context usage">
          <div style={{ width: `${pct}%` }} />
        </div>
        <span>{usage ? `${pct}% · ${usage.input_tokens + usage.output_tokens} tok` : "—"}</span>
      </div>
      <ChatInput session={session} conversation={conversationId} busy={conv.busy} />
    </div>
  );
}
