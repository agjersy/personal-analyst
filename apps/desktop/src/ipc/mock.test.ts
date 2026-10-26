import { describe, expect, it } from "vitest";
import { MockBackend } from "./mock";
import type { SessionEvent, SessionInfo } from "./types";

function fast(): MockBackend {
  return new MockBackend({ latency: [0, 0], timeScale: 0, seedSession: false });
}

async function session(b: MockBackend): Promise<{ info: SessionInfo; events: SessionEvent[] }> {
  const info = await b.invoke("session_create", { repo_abs: "/home/user/projects/decay-analysis", target: "local" });
  const events: SessionEvent[] = [];
  await b.subscribe(info.id, (e) => events.push(e));
  return { info, events };
}

function waitFor<T>(check: () => T | undefined, timeoutMs = 3000): Promise<T> {
  return new Promise((resolve, reject) => {
    const start = Date.now();
    const tick = () => {
      const v = check();
      if (v !== undefined) resolve(v);
      else if (Date.now() - start > timeoutMs) reject(new Error("timeout waiting for condition"));
      else setTimeout(tick, 2);
    };
    tick();
  });
}

describe("MockBackend", () => {
  it("creates a session with the fixture repo and index", async () => {
    const b = fast();
    const { info, events } = await session(b);
    expect(info.open).toBe(true);
    expect(info.conversations).toHaveLength(1);
    await waitFor(() => events.find((e) => e.type === "kernel_state" && e.state === "ready"));
    const tree = await b.invoke("fs_tree", { session: info.id });
    expect(tree.map((e) => e.path)).toEqual(expect.arrayContaining([".analyst", "analysis", "scripts", "data", "notes.md"]));
    const index = await b.invoke("index_get", { session: info.id });
    const decay = index.scripts.find((s) => s.name === "decay_sim");
    expect(decay?.options).toHaveLength(5);
    expect(decay?.options.map((o) => o.dest)).toEqual(["n", "tau", "amplitude", "noise", "seed"]);
    expect(decay?.options[1]).toMatchObject({ type: "float", default: 2.5, help: "true decay constant" });
    const single = index.library.find((f) => f.qualname === "single_exp");
    expect(single?.has_math).toBe(true);
    expect(single?.callers.length).toBeGreaterThan(0);
  });

  it("reports one warning and one error on scripts/bad.py", async () => {
    const b = fast();
    const { info, events } = await session(b);
    await waitFor(() => events.find((e) => e.type === "kernel_state" && e.state === "ready"));
    const diags = await b.invoke("check_run", { session: info.id, paths: ["scripts/bad.py", "scripts/decay_sim.py"] });
    expect(diags.filter((d) => d.path === "scripts/bad.py").map((d) => d.severity).sort()).toEqual(["error", "warn"]);
    expect(diags.filter((d) => d.path === "scripts/decay_sim.py")).toHaveLength(0);
    expect(events.find((e) => e.type === "diagnostics")).toBeDefined();
  });

  it("streams a run with output, an SVG figure and run_done", async () => {
    const b = fast();
    const { info, events } = await session(b);
    await waitFor(() => events.find((e) => e.type === "kernel_state" && e.state === "ready"));
    const run = await b.invoke("run_start", { session: info.id, script: "scripts/decay_sim.py", args: ["--tau", "1.5"] });
    expect(run.meta.status).toBe("running");
    const done = await waitFor(() => events.find((e) => e.type === "run_done" && e.run.meta.run_id === run.meta.run_id));
    expect(done.type === "run_done" && done.run.meta.exit_code).toBe(0);
    const outputs = events.filter((e) => e.type === "run_output");
    expect(outputs.length).toBeGreaterThanOrEqual(3);
    const art = events.find((e) => e.type === "run_artifact" && e.figure !== null);
    expect(art && art.type === "run_artifact" && art.figure?.format).toBe("svg");
    const data = await b.invoke("figure_data", { session: info.id, path: art && art.type === "run_artifact" ? art.artifact.path : "" });
    expect(data.mime).toBe("image/svg+xml");
    expect(atob(data.base64)).toContain("<svg");
    const groups = await b.invoke("figures_list", { session: info.id });
    expect(groups[0]?.figures[0]?.name).toBe("decay-simulation");
    const full = await b.invoke("run_get", { session: info.id, run_id: run.meta.run_id });
    expect(full.stdout).toContain("tau_fit");
  });

  it("fails with a traceback when args contain --fail", async () => {
    const b = fast();
    const { info, events } = await session(b);
    await waitFor(() => events.find((e) => e.type === "kernel_state" && e.state === "ready"));
    const run = await b.invoke("run_start", { session: info.id, script: "scripts/decay_sim.py", args: ["--fail"] });
    const done = await waitFor(() => events.find((e) => e.type === "run_done" && e.run.meta.run_id === run.meta.run_id));
    expect(done.type === "run_done" && done.run.meta.exit_code).toBe(1);
    expect(done.type === "run_done" && done.run.stderr_tail).toContain("Traceback");
  });

  it("build mode pauses on a proposal and continues after accept with a run and summary", async () => {
    const b = fast();
    const { info, events } = await session(b);
    await waitFor(() => events.find((e) => e.type === "kernel_state" && e.state === "ready"));
    const conv = info.active_conversation;
    await b.invoke("chat_send", { session: info.id, conversation: conv, text: "Add a double exponential model" });
    const proposal = await waitFor(() => events.find((e) => e.type === "proposal"));
    if (proposal.type !== "proposal") throw new Error("unreachable");
    expect(proposal.proposal.kind).toBe("edit");
    expect(proposal.proposal.path).toBe("analysis/models.py");
    expect(proposal.proposal.proposed).toContain("def double_exp");
    expect(proposal.proposal.diff).toContain("+def double_exp");
    await expect(b.invoke("chat_send", { session: info.id, conversation: conv, text: "again" })).rejects.toMatchObject({ code: "invalid" });
    expect(events.find((e) => e.type === "turn_done")).toBeUndefined();
    const edited = proposal.proposal.proposed!.replace("Sum of two exponential decays", "Two exponentials");
    await b.invoke("proposal_respond", { session: info.id, proposal_id: proposal.proposal.id, decision: { decision: "accept", content: edited } });
    const done = await waitFor(() => events.find((e) => e.type === "turn_done"));
    if (done.type !== "turn_done") throw new Error("unreachable");
    expect(done.summary.files_changed).toEqual(["analysis/models.py"]);
    expect(done.summary.runs).toHaveLength(1);
    expect(done.summary.figures.length).toBeGreaterThanOrEqual(1);
    expect(events.some((e) => e.type === "run_started")).toBe(true);
    expect(events.some((e) => e.type === "proposal_resolved" && e.decision === "accept")).toBe(true);
    expect(events.some((e) => e.type === "files_changed" && e.paths.includes("analysis/models.py"))).toBe(true);
    expect(events.some((e) => e.type === "usage")).toBe(true);
    const file = await b.invoke("fs_read", { session: info.id, path: "analysis/models.py" });
    expect(file.content).toContain("Two exponentials");
    const text = events.filter((e) => e.type === "agent_text").map((e) => (e.type === "agent_text" ? e.delta : "")).join("");
    expect(text).toContain("your edited version");
    await expect(b.invoke("proposal_respond", { session: info.id, proposal_id: proposal.proposal.id, decision: { decision: "reject" } })).rejects.toMatchObject({
      code: "not_found",
    });
    const index = await b.invoke("index_get", { session: info.id });
    expect(index.library.some((f) => f.qualname === "double_exp")).toBe(true);
  });

  it("rejecting a proposal ends the turn without changes", async () => {
    const b = fast();
    const { info, events } = await session(b);
    await waitFor(() => events.find((e) => e.type === "kernel_state" && e.state === "ready"));
    const conv = info.active_conversation;
    await b.invoke("chat_send", { session: info.id, conversation: conv, text: "Add a model" });
    const proposal = await waitFor(() => events.find((e) => e.type === "proposal"));
    if (proposal.type !== "proposal") throw new Error("unreachable");
    await b.invoke("proposal_respond", { session: info.id, proposal_id: proposal.proposal.id, decision: { decision: "reject", reason: "not now" } });
    const done = await waitFor(() => events.find((e) => e.type === "turn_done"));
    expect(done.type === "turn_done" && done.summary.files_changed).toEqual([]);
    const file = await b.invoke("fs_read", { session: info.id, path: "analysis/models.py" });
    expect(file.content).not.toContain("double_exp");
  });

  it("run mode runs the script twice; plan mode streams text only; 'error' produces turn_error", async () => {
    const b = fast();
    const { info, events } = await session(b);
    await waitFor(() => events.find((e) => e.type === "kernel_state" && e.state === "ready"));
    const conv = info.active_conversation;
    await b.invoke("chat_set_mode", { session: info.id, conversation: conv, mode: "run" });
    await b.invoke("chat_send", { session: info.id, conversation: conv, text: "Sweep tau" });
    const done = await waitFor(() => events.find((e) => e.type === "turn_done"));
    expect(done.type === "turn_done" && done.summary.runs).toHaveLength(2);
    expect(events.filter((e) => e.type === "run_started")).toHaveLength(2);
    expect(events.some((e) => e.type === "proposal")).toBe(false);

    events.length = 0;
    await b.invoke("chat_set_mode", { session: info.id, conversation: conv, mode: "plan" });
    await b.invoke("chat_send", { session: info.id, conversation: conv, text: "Plan a fit" });
    await waitFor(() => events.find((e) => e.type === "turn_done"));
    expect(events.some((e) => e.type === "agent_text")).toBe(true);
    expect(events.some((e) => e.type === "run_started" || e.type === "proposal")).toBe(false);

    events.length = 0;
    await b.invoke("chat_send", { session: info.id, conversation: conv, text: "please trigger an error" });
    const err = await waitFor(() => events.find((e) => e.type === "turn_error"));
    expect(err.type === "turn_error" && err.message).toMatch(/error/);
    const transcript = await b.invoke("conversation_transcript", { session: info.id, conversation: conv });
    expect(transcript.filter((t) => t.type === "user")).toHaveLength(3);
  });

  it("chat_cancel rejects pending proposals", async () => {
    const b = fast();
    const { info, events } = await session(b);
    await waitFor(() => events.find((e) => e.type === "kernel_state" && e.state === "ready"));
    const conv = info.active_conversation;
    await b.invoke("chat_send", { session: info.id, conversation: conv, text: "Add a model" });
    await waitFor(() => events.find((e) => e.type === "proposal"));
    await b.invoke("chat_cancel", { session: info.id, conversation: conv });
    expect(events.some((e) => e.type === "proposal_resolved" && e.decision === "reject")).toBe(true);
    await waitFor(() => events.find((e) => e.type === "turn_done"));
  });

  it("fs_write enforces expected_hash and emits files_changed and diagnostics", async () => {
    const b = fast();
    const { info, events } = await session(b);
    const file = await b.invoke("fs_read", { session: info.id, path: "scripts/bad.py" });
    await expect(b.invoke("fs_write", { session: info.id, path: "scripts/bad.py", content: "x", expected_hash: "0".repeat(64) })).rejects.toMatchObject({ code: "conflict" });
    const fixed = file.content.replace("import argparse", "import argparse\nimport numpy as np").replace('"/Users/me/data/run3.csv"', "args.path");
    const res = await b.invoke("fs_write", { session: info.id, path: "scripts/bad.py", content: fixed, expected_hash: file.hash });
    expect(res.hash).not.toBe(file.hash);
    expect(events.some((e) => e.type === "files_changed" && e.paths.includes("scripts/bad.py"))).toBe(true);
    const diag = await waitFor(() => events.find((e) => e.type === "diagnostics" && e.paths.includes("scripts/bad.py")));
    expect(diag.type === "diagnostics" && diag.diagnostics).toHaveLength(0);
  });

  it("terminal echoes input and answers ls and python --version", async () => {
    const b = fast();
    const { info, events } = await session(b);
    await waitFor(() => events.find((e) => e.type === "kernel_state" && e.state === "ready"));
    const id = await b.invoke("terminal_open", { session: info.id, cols: 80, rows: 24 });
    const text = () =>
      events
        .filter((e) => e.type === "terminal_data" && e.terminal_id === id)
        .map((e) => (e.type === "terminal_data" ? e.data : ""))
        .join("");
    await waitFor(() => (text().includes("$ ") ? true : undefined));
    await b.invoke("terminal_write", { terminal_id: id, data: "ls\r" });
    await waitFor(() => (text().includes("scripts") ? true : undefined));
    await b.invoke("terminal_write", { terminal_id: id, data: "python --version\r" });
    await waitFor(() => (text().includes("Python 3.12") ? true : undefined));
    expect(text()).toContain("ls");
    await b.invoke("terminal_close", { terminal_id: id });
    expect(events.some((e) => e.type === "terminal_exit" && e.terminal_id === id)).toBe(true);
  });

  it("lists models and targets and runs package tasks", async () => {
    const b = fast();
    const models = await b.invoke("models_list", {});
    expect(models.map((m) => m.local)).toEqual([true, false]);
    const targets = await b.invoke("targets_list", {});
    expect(targets.map((t) => t.kind)).toEqual(["local", "ssh"]);
    const { info, events } = await session(b);
    const task = await b.invoke("env_add", { session: info.id, packages: ["lmfit"] });
    await waitFor(() => events.find((e) => e.type === "task_done" && e.task_id === task));
    expect(events.some((e) => e.type === "task_log" && e.task_id === task)).toBe(true);
    expect(events.some((e) => e.type === "packages_changed")).toBe(true);
    const pkgs = await b.invoke("env_packages", { session: info.id });
    expect(pkgs.some((p) => p.name === "lmfit")).toBe(true);
  });
});
