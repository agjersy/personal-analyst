import { beforeEach, describe, expect, it } from "vitest";
import type { RunView, SessionEvent, SessionInfo } from "../ipc";
import { useChat } from "./chat";
import { useDiagnostics } from "./diagnostics";
import { applyEvent } from "./dispatcher";
import { useFiles } from "./files";
import { useRuns } from "./runs";
import { useSessions } from "./sessions";
import { useTasks } from "./tasks";
import { attachTerminalSink, deliverTerminalData, useTerminals } from "./terminals";

const S = "s1";
const C = "s1-c1";

function run(id: string, status: RunView["meta"]["status"] = "running"): RunView {
  return {
    meta: { run_id: id, script: "scripts/decay_sim.py", args: [], status, exit_code: status === "done" ? 0 : null, started_at: "2026-10-26T00:00:00Z", finished_at: null, out_dir: `out/decay_sim/${id}`, sandbox: "bwrap", tree_hash: null, pid: 1, host: "localhost" },
    target: "local",
    artifacts: [],
    stdout_tail: "",
    stderr_tail: "",
  };
}

function info(): SessionInfo {
  return {
    id: S,
    name: "repo",
    repo_abs: "/r",
    target: "local",
    model: null,
    created_at: "",
    last_opened_at: "",
    open: true,
    policy: { edits: "review", runs: "auto", packages: "ask", shell: "ask", network: "deny", git: "ask", protected: [], max_steps: 10 },
    kernel: "ready",
    sandbox: null,
    python_abs: null,
    conversations: [{ id: C, title: "c", model: null, mode: "build", created_at: "", turns: 0, busy: false }],
    active_conversation: C,
    target_kind: "local",
    target_host: null,
  };
}

beforeEach(() => {
  useChat.setState({ conversations: {}, proposals: {} });
  useRuns.setState({ runs: {}, output: {}, figures: {}, activeFigure: {}, pinned: {} });
  useDiagnostics.setState({ byPath: {} });
  useFiles.setState({ indexStale: {}, trees: {}, buffers: {} });
  useSessions.setState({ infos: {}, kernel: {}, sessions: [] });
  useTasks.setState({ tasks: {} });
  useTerminals.setState({ terminals: {}, active: {} });
  useSessions.getState().setInfo(info());
});

describe("applyEvent", () => {
  it("streams assistant text into one item per turn and finalises on turn_done", () => {
    applyEvent(S, { type: "turn_started", conversation: C, turn: "t1" });
    applyEvent(S, { type: "agent_text", conversation: C, turn: "t1", delta: "Hello " });
    applyEvent(S, { type: "agent_text", conversation: C, turn: "t1", delta: "world" });
    let conv = useChat.getState().conversations[C]!;
    expect(conv.busy).toBe(true);
    expect(conv.items).toHaveLength(1);
    expect(conv.items[0]).toMatchObject({ kind: "assistant", text: "Hello world", streaming: true });
    applyEvent(S, { type: "turn_done", conversation: C, turn: "t1", summary: { text: "done", files_changed: [], runs: [], figures: [] } });
    conv = useChat.getState().conversations[C]!;
    expect(conv.busy).toBe(false);
    expect(conv.items[0]).toMatchObject({ kind: "assistant", streaming: false });
    expect(conv.items[1]).toMatchObject({ kind: "summary" });
  });

  it("pairs tool calls with their results", () => {
    applyEvent(S, { type: "agent_tool_call", conversation: C, turn: "t1", id: "x", name: "lib.index", arguments: {} });
    applyEvent(S, { type: "agent_tool_result", conversation: C, turn: "t1", id: "x", ok: true, summary: "2 functions" });
    const item = useChat.getState().conversations[C]!.items[0];
    expect(item).toMatchObject({ kind: "tool", name: "lib.index", result: { ok: true, summary: "2 functions" } });
  });

  it("records proposals and their resolution", () => {
    const proposal = { id: "p1", conversation: C, kind: "edit" as const, path: "a.py", summary: "s", original: "a", proposed: "b", diff: null, command: null, packages: null, message: null, in_scope: true, reason: "review" as const };
    applyEvent(S, { type: "proposal", proposal });
    expect(useChat.getState().proposals.p1).toMatchObject({ status: "pending", session: S });
    expect(useChat.getState().conversations[C]!.items[0]).toMatchObject({ kind: "proposal", proposal_id: "p1" });
    applyEvent(S, { type: "proposal_resolved", proposal_id: "p1", decision: "accept", by: "user" });
    expect(useChat.getState().proposals.p1).toMatchObject({ status: "accept", by: "user" });
  });

  it("tracks runs, output, artifacts and figures, and attaches run cards to the busy conversation", () => {
    applyEvent(S, { type: "turn_started", conversation: C, turn: "t1" });
    applyEvent(S, { type: "run_started", run: run("r1") });
    applyEvent(S, { type: "run_output", run_id: "r1", stream: "stdout", chunk: "a" });
    applyEvent(S, { type: "run_output", run_id: "r1", stream: "stdout", chunk: "b" });
    applyEvent(S, { type: "run_output", run_id: "r1", stream: "stderr", chunk: "e" });
    const figure = { run_id: "r1", script: "scripts/decay_sim.py", path: "out/f.svg", name: "f", format: "svg" as const, sidecar: null, created_at: "", size: 1 };
    applyEvent(S, { type: "run_artifact", run_id: "r1", artifact: { kind: "figure", path: "out/f.svg", size: 1, created_at: "", rendered: true }, figure });
    applyEvent(S, { type: "run_done", run: run("r1", "done") });
    const st = useRuns.getState();
    expect(st.runs[S]).toHaveLength(1);
    expect(st.runs[S]![0]!.meta.status).toBe("done");
    expect(st.runs[S]![0]!.artifacts).toHaveLength(1);
    expect(st.output.r1).toEqual({ stdout: "ab", stderr: "e" });
    expect(st.figures[S]![0]!.figures[0]!.name).toBe("f");
    expect(st.figures[S]![0]!.run.meta.status).toBe("done");
    expect(st.activeFigure[S]).toEqual({ name: "f", run_id: "r1" });
    expect(useChat.getState().conversations[C]!.items.some((i) => i.kind === "run" && i.run_id === "r1")).toBe(true);
  });

  it("keeps a pinned figure selected when a newer run arrives", () => {
    useRuns.getState().setFigures(S, []);
    const fig = (id: string) => ({ run_id: id, script: "s", path: `out/${id}.svg`, name: "f", format: "svg" as const, sidecar: null, created_at: "", size: 1 });
    applyEvent(S, { type: "run_started", run: run("r1") });
    applyEvent(S, { type: "run_artifact", run_id: "r1", artifact: { kind: "figure", path: "out/r1.svg", size: 1, created_at: "", rendered: true }, figure: fig("r1") });
    useRuns.getState().togglePin(S, { name: "f", run_id: "r1" });
    applyEvent(S, { type: "run_started", run: run("r2") });
    applyEvent(S, { type: "run_artifact", run_id: "r2", artifact: { kind: "figure", path: "out/r2.svg", size: 1, created_at: "", rendered: true }, figure: fig("r2") });
    expect(useRuns.getState().activeFigure[S]).toEqual({ name: "f", run_id: "r1" });
  });

  it("replaces diagnostics per path", () => {
    const d = (path: string, line: number) => ({ path, line, col: 0, rule: "r", severity: "warn" as const, message: "m", hint: null, related: [] });
    applyEvent(S, { type: "diagnostics", paths: ["a.py", "b.py"], diagnostics: [d("a.py", 1), d("b.py", 2)] });
    applyEvent(S, { type: "diagnostics", paths: ["a.py"], diagnostics: [] });
    expect(useDiagnostics.getState().byPath[S]).toEqual({ "b.py": [d("b.py", 2)] });
  });

  it("marks the index stale and invalidates directories on files_changed", () => {
    useFiles.getState().setTree(S, "scripts", []);
    applyEvent(S, { type: "files_changed", paths: ["scripts/new.py"] });
    expect(useFiles.getState().indexStale[S]).toBe(true);
    expect(useFiles.getState().trees[S]!["scripts"]).toBeUndefined();
    applyEvent(S, { type: "index_changed" });
    expect(useFiles.getState().indexStale[S]).toBe(true);
  });

  it("routes terminal data to sinks and buffers it before a sink attaches", () => {
    deliverTerminalData("t1", "early");
    const got: string[] = [];
    const detach = attachTerminalSink("t1", (d) => got.push(d));
    applyEvent(S, { type: "terminal_data", terminal_id: "t1", data: "late" });
    expect(got).toEqual(["early", "late"]);
    useTerminals.getState().add(S, "t1", "Terminal 1");
    applyEvent(S, { type: "terminal_exit", terminal_id: "t1", code: 0 });
    expect(useTerminals.getState().terminals[S]![0]!.exit).toBe(0);
    detach();
  });

  it("updates kernel state, tasks and session info", () => {
    applyEvent(S, { type: "kernel_state", state: "starting", message: null });
    expect(useSessions.getState().kernel[S]).toEqual({ state: "starting", message: null });
    applyEvent(S, { type: "task_log", task_id: "k", line: "one" });
    applyEvent(S, { type: "task_done", task_id: "k", ok: true, message: null });
    expect(useTasks.getState().tasks.k).toMatchObject({ log: ["one"], done: true, ok: true });
    const next = { ...info(), name: "renamed" };
    applyEvent(S, { type: "session_changed", info: next });
    expect(useSessions.getState().infos[S]!.name).toBe("renamed");
    expect(useSessions.getState().sessions[0]!.name).toBe("renamed");
    applyEvent(S, { type: "usage", conversation: C, input_tokens: 10, output_tokens: 5, context_window: 100, context_used: 15 });
    expect(useChat.getState().conversations[C]!.usage?.context_used).toBe(15);
    const events: SessionEvent[] = [{ type: "turn_error", conversation: C, turn: "t", message: "boom" }];
    for (const e of events) applyEvent(S, e);
    expect(useChat.getState().conversations[C]!.items.at(-1)).toMatchObject({ kind: "error", message: "boom" });
  });
});
