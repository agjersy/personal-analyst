import { useEffect, useRef } from "react";
import type { Attachment, ConversationId, SessionId } from "../../ipc";
import { cancelChat, sendChat } from "../../state/actions";
import { useChat } from "../../state/chat";
import { activeEditorPath } from "../workbench/controller";
import { editorSelection } from "../editor/registry";

interface ChatInputProps {
  session: SessionId;
  conversation: ConversationId;
  busy: boolean;
}

function describe(a: Attachment): string {
  if (a.kind === "selection") return `${a.path}:${a.start_line}-${a.end_line}`;
  return `${a.kind}: ${a.path.split("/").pop()}`;
}

function currentSelection(): Attachment | null {
  const path = activeEditorPath();
  return path ? editorSelection(path) : null;
}

const NO_ATTACHMENTS: Attachment[] = [];

export function ChatInput({ session, conversation, busy }: ChatInputProps) {
  const draft = useChat((s) => s.drafts[conversation] ?? "");
  const attachments = useChat((s) => s.pendingAttachments[conversation] ?? NO_ATTACHMENTS);
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onAttach = () => {
      const a = currentSelection();
      if (a) useChat.getState().addAttachment(conversation, a);
    };
    el.addEventListener("analyst:attach-selection", onAttach);
    return () => el.removeEventListener("analyst:attach-selection", onAttach);
  }, [conversation]);

  const send = () => {
    const text = draft.trim();
    if (!text || busy) return;
    void sendChat(session, conversation, text, attachments);
  };

  const attachSelection = () => {
    const a = currentSelection();
    if (a) useChat.getState().addAttachment(conversation, a);
    else {
      const path = activeEditorPath();
      if (path) useChat.getState().addAttachment(conversation, { kind: "file", path });
    }
  };

  return (
    <div className="chat-input">
      {attachments.length ? (
        <div className="attachments">
          {attachments.map((a, i) => (
            <span key={`${a.kind}-${a.path}-${i}`} className="badge accent">
              {describe(a)}{" "}
              <button type="button" className="ghost small" onClick={() => useChat.getState().removeAttachment(conversation, i)} aria-label={`Remove attachment ${describe(a)}`}>
                ×
              </button>
            </span>
          ))}
        </div>
      ) : null}
      <textarea
        ref={ref}
        data-chat-input
        aria-label="Message the agent"
        placeholder="Ask for an analysis… (⌘/Ctrl+Enter to send)"
        value={draft}
        onChange={(e) => useChat.getState().setDraft(conversation, e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            send();
          }
        }}
      />
      <div className="row">
        <button type="button" className="small" onClick={attachSelection} title="Attach the editor selection, or the active file">
          + Selection
        </button>
        <span className="grow" />
        {busy ? (
          <button type="button" className="small danger" onClick={() => void cancelChat(session, conversation)}>
            Cancel
          </button>
        ) : null}
        <button type="button" className="small primary" onClick={send} disabled={busy || !draft.trim()} data-testid="chat-send">
          Send
        </button>
      </div>
    </div>
  );
}
