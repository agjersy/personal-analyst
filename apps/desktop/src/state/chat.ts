import { create } from "zustand";
import type { Attachment, ConversationId, ProposalId, ProposalView, RunId, SessionId, TurnId, TurnSummary } from "../ipc";

export type ProposalStatus = "pending" | "accept" | "reject" | "auto";

export type ChatItem =
  | { kind: "user"; id: string; text: string; attachments: Attachment[] }
  | { kind: "assistant"; id: string; turn: TurnId; text: string; streaming: boolean }
  | { kind: "tool"; id: string; turn: TurnId; name: string; arguments: unknown; result: { ok: boolean; summary: string } | null }
  | { kind: "proposal"; id: string; proposal_id: ProposalId }
  | { kind: "run"; id: string; run_id: RunId }
  | { kind: "summary"; id: string; turn: TurnId; summary: TurnSummary }
  | { kind: "error"; id: string; turn: TurnId; message: string };

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  context_window: number;
  context_used: number;
}

export interface ConversationState {
  items: ChatItem[];
  busy: boolean;
  turn: TurnId | null;
  usage: Usage | null;
  loaded: boolean;
}

export interface ProposalRecord {
  session: SessionId;
  proposal: ProposalView;
  status: ProposalStatus;
  by: "user" | "policy" | null;
}

interface ChatState {
  conversations: Record<ConversationId, ConversationState>;
  proposals: Record<ProposalId, ProposalRecord>;
  drafts: Record<ConversationId, string>;
  pendingAttachments: Record<ConversationId, Attachment[]>;
  ensure(conversation: ConversationId): void;
  setItems(conversation: ConversationId, items: ChatItem[]): void;
  addUser(conversation: ConversationId, text: string, attachments: Attachment[]): void;
  turnStarted(conversation: ConversationId, turn: TurnId): void;
  appendText(conversation: ConversationId, turn: TurnId, delta: string): void;
  toolCall(conversation: ConversationId, turn: TurnId, id: string, name: string, args: unknown): void;
  toolResult(conversation: ConversationId, id: string, ok: boolean, summary: string): void;
  addProposal(session: SessionId, proposal: ProposalView): void;
  resolveProposal(proposal_id: ProposalId, status: Exclude<ProposalStatus, "pending">, by: "user" | "policy"): void;
  addRun(conversation: ConversationId, run_id: RunId): void;
  turnDone(conversation: ConversationId, turn: TurnId, summary: TurnSummary): void;
  turnError(conversation: ConversationId, turn: TurnId, message: string): void;
  setUsage(conversation: ConversationId, usage: Usage): void;
  setBusy(conversation: ConversationId, busy: boolean): void;
  setDraft(conversation: ConversationId, text: string): void;
  addAttachment(conversation: ConversationId, attachment: Attachment): void;
  clearAttachments(conversation: ConversationId): void;
  removeAttachment(conversation: ConversationId, index: number): void;
}

const EMPTY: ConversationState = { items: [], busy: false, turn: null, usage: null, loaded: false };

let counter = 0;
function nextId(prefix: string): string {
  return `${prefix}${++counter}`;
}

function update(
  s: ChatState,
  conversation: ConversationId,
  fn: (c: ConversationState) => Partial<ConversationState>,
): Pick<ChatState, "conversations"> {
  const cur = s.conversations[conversation] ?? EMPTY;
  return { conversations: { ...s.conversations, [conversation]: { ...cur, ...fn(cur) } } };
}

function finishStreaming(items: ChatItem[]): ChatItem[] {
  return items.map((it) => (it.kind === "assistant" && it.streaming ? { ...it, streaming: false } : it));
}

export const useChat = create<ChatState>((set) => ({
  conversations: {},
  proposals: {},
  drafts: {},
  pendingAttachments: {},
  ensure: (conversation) => set((s) => (s.conversations[conversation] ? {} : update(s, conversation, () => ({})))),
  setItems: (conversation, items) => set((s) => update(s, conversation, () => ({ items, loaded: true }))),
  addUser: (conversation, text, attachments) =>
    set((s) => update(s, conversation, (c) => ({ items: [...c.items, { kind: "user", id: nextId("u"), text, attachments }], busy: true }))),
  turnStarted: (conversation, turn) => set((s) => update(s, conversation, () => ({ busy: true, turn }))),
  appendText: (conversation, turn, delta) =>
    set((s) =>
      update(s, conversation, (c) => {
        const last = c.items[c.items.length - 1];
        if (last && last.kind === "assistant" && last.turn === turn && last.streaming) {
          return { items: [...c.items.slice(0, -1), { ...last, text: last.text + delta }] };
        }
        return { items: [...finishStreaming(c.items), { kind: "assistant", id: nextId("a"), turn, text: delta, streaming: true }] };
      }),
    ),
  toolCall: (conversation, turn, id, name, args) =>
    set((s) =>
      update(s, conversation, (c) => ({
        items: [...finishStreaming(c.items), { kind: "tool", id: `tool:${id}`, turn, name, arguments: args, result: null }],
      })),
    ),
  toolResult: (conversation, id, ok, summary) =>
    set((s) =>
      update(s, conversation, (c) => ({
        items: c.items.map((it) => (it.kind === "tool" && it.id === `tool:${id}` ? { ...it, result: { ok, summary } } : it)),
      })),
    ),
  addProposal: (session, proposal) =>
    set((s) => ({
      proposals: { ...s.proposals, [proposal.id]: { session, proposal, status: "pending", by: null } },
      ...update(s, proposal.conversation, (c) => ({
        items: [...finishStreaming(c.items), { kind: "proposal", id: `proposal:${proposal.id}`, proposal_id: proposal.id }],
      })),
    })),
  resolveProposal: (proposal_id, status, by) =>
    set((s) => {
      const rec = s.proposals[proposal_id];
      if (!rec) return {};
      return { proposals: { ...s.proposals, [proposal_id]: { ...rec, status, by } } };
    }),
  addRun: (conversation, run_id) =>
    set((s) =>
      update(s, conversation, (c) =>
        c.items.some((it) => it.kind === "run" && it.run_id === run_id)
          ? {}
          : { items: [...finishStreaming(c.items), { kind: "run", id: `run:${run_id}`, run_id }] },
      ),
    ),
  turnDone: (conversation, turn, summary) =>
    set((s) =>
      update(s, conversation, (c) => ({
        items: [...finishStreaming(c.items), { kind: "summary", id: nextId("s"), turn, summary }],
        busy: false,
        turn: null,
      })),
    ),
  turnError: (conversation, turn, message) =>
    set((s) =>
      update(s, conversation, (c) => ({
        items: [...finishStreaming(c.items), { kind: "error", id: nextId("e"), turn, message }],
        busy: false,
        turn: null,
      })),
    ),
  setUsage: (conversation, usage) => set((s) => update(s, conversation, () => ({ usage }))),
  setBusy: (conversation, busy) => set((s) => update(s, conversation, () => ({ busy }))),
  setDraft: (conversation, text) => set((s) => ({ drafts: { ...s.drafts, [conversation]: text } })),
  addAttachment: (conversation, attachment) =>
    set((s) => ({ pendingAttachments: { ...s.pendingAttachments, [conversation]: [...(s.pendingAttachments[conversation] ?? []), attachment] } })),
  clearAttachments: (conversation) => set((s) => ({ pendingAttachments: { ...s.pendingAttachments, [conversation]: [] } })),
  removeAttachment: (conversation, index) =>
    set((s) => ({
      pendingAttachments: { ...s.pendingAttachments, [conversation]: (s.pendingAttachments[conversation] ?? []).filter((_, i) => i !== index) },
    })),
}));

export function useConversation(conversation: ConversationId | null): ConversationState {
  return useChat((s) => (conversation ? (s.conversations[conversation] ?? EMPTY) : EMPTY));
}

export function useProposal(proposal_id: ProposalId | null): ProposalRecord | undefined {
  return useChat((s) => (proposal_id ? s.proposals[proposal_id] : undefined));
}
