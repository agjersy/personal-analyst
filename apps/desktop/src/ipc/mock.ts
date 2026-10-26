// MockBackend: the renderer-only backend described in docs/IPC.md section 6.
// Everything lives in memory; latency and streaming are simulated so loading
// states are visible. Selected automatically when the Tauri API is absent.
import type { Backend } from "./backend";
import type { Commands } from "./commands";
import { unifiedDiff } from "./mock/diff";
import { decaySvg, residualsSvg, simParamsFromArgs, type SimParams } from "./mock/figure";
import { DOUBLE_EXP_SNIPPET, fixtureFiles } from "./mock/fixtures";
import { extractEquations, indexLibrary, parseScript } from "./mock/indexer";
import type {
  Artifact,
  ConversationId,
  ConversationSummary,
  Diagnostic,
  FigureGroup,
  FigureView,
  FileEntry,
  IpcError,
  Mode,
  ModelEntry,
  PackageInfo,
  PolicyView,
  ProposalDecision,
  ProposalId,
  ProposalView,
  RunId,
  RunView,
  SessionEvent,
  SessionId,
  SessionInfo,
  SessionSummary,
  TargetInfo,
  TaskId,
  TerminalId,
  TranscriptEvent,
  TurnId,
} from "./types";

export interface MockOptions {
  /** [min, max] latency per command in ms; [0, 0] for tests. */
  latency?: [number, number];
  /** Scale applied to streaming delays (1 = real time, 0 = as fast as possible). */
  timeScale?: number;
  /** Start with one existing (closed) session. Default true. */
  seedSession?: boolean;
}

interface MockRun {
  view: RunView;
  stdout: string;
  stderr: string;
  figures: FigureView[];
  cancelled: boolean;
  timers: ReturnType<typeof setTimeout>[];
}

interface MockConversation {
  summary: ConversationSummary;
  transcript: TranscriptEvent[];
  cancelled: boolean;
}

interface PendingProposal {
  proposal: ProposalView;
  resolve: (d: ProposalDecision) => void;
}

interface MockSession {
  info: SessionInfo;
  files: Map<string, string>;
  hashes: Map<string, string>;
  runs: MockRun[];
  conversations: Map<ConversationId, MockConversation>;
  pending: Map<ProposalId, PendingProposal>;
  layout: string | null;
  packages: PackageInfo[];
  usage: { input: number; output: number };
  counters: { turn: number; run: number; conv: number; proposal: number; tool: number };
}

interface MockTerminal {
  session: SessionId;
  line: string;
  busy: boolean;
}

const MODELS: ModelEntry[] = [
  {
    name: "qwen2.5-coder:32b",
    provider: "ollama",
    id: "qwen2.5-coder:32b",
    kind: "openai-compatible",
    local: true,
    capabilities: { context_window: 32768, tools: "text-protocol", vision: false },
  },
  {
    name: "claude-sonnet-4-5",
    provider: "anthropic",
    id: "claude-sonnet-4-5",
    kind: "anthropic",
    local: false,
    capabilities: { context_window: 200000, tools: "native", vision: true },
  },
];

const TARGETS: TargetInfo[] = [
  { name: "local", kind: "local", host: null, remote_root: null, sandbox: "bwrap", data_dirs: ["data"], bootstrapped: true },
  {
    name: "lab-gpu",
    kind: "ssh",
    host: "lab-gpu.example.edu",
    remote_root: "~/analyst/mirrors",
    sandbox: "bwrap",
    data_dirs: ["/data/lab/run3"],
    bootstrapped: false,
  },
];

const DEFAULT_POLICY: PolicyView = {
  edits: "review",
  runs: "auto",
  packages: "ask",
  shell: "ask",
  network: "deny",
  git: "ask",
  protected: ["data/**", ".analyst/**"],
  max_steps: 40,
};

const DEFAULT_PACKAGES: PackageInfo[] = [
  { name: "numpy", version: "2.1.2", locked: true },
  { name: "scipy", version: "1.14.1", locked: true },
  { name: "matplotlib", version: "3.9.2", locked: true },
  { name: "pandas", version: "2.2.3", locked: false },
];

function ipcError(code: IpcError["code"], message: string, detail?: unknown): IpcError {
  return detail === undefined ? { code, message } : { code, message, detail };
}

function nowIso(): string {
  return new Date().toISOString();
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

const encoder = new TextEncoder();

async function sha256(text: string): Promise<string> {
  const bytes = encoder.encode(text);
  const subtle = globalThis.crypto?.subtle;
  if (subtle) {
    const buf = await subtle.digest("SHA-256", bytes);
    return Array.from(new Uint8Array(buf), (b) => b.toString(16).padStart(2, "0")).join("");
  }
  // Non-secure contexts: FNV-1a over the bytes, 64 hex chars for shape parity.
  let h = 0x811c9dc5;
  for (const b of bytes) h = Math.imul(h ^ b, 0x01000193) >>> 0;
  return h.toString(16).padStart(8, "0").repeat(8);
}

function base64Utf8(text: string): string {
  const bytes = encoder.encode(text);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

function runIdNow(seq: number): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  const stamp = `${d.getUTCFullYear()}${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}-${p(d.getUTCHours())}${p(d.getUTCMinutes())}${p(d.getUTCSeconds())}`;
  return `${stamp}-${(0x100000 + ((seq * 2654435761) >>> 0) % 0xefffff).toString(16).slice(0, 6)}`;
}

function tail(s: string, n = 4096): string {
  return s.length > n ? s.slice(s.length - n) : s;
}

function basename(p: string): string {
  const parts = p.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

function platform(): "macos" | "linux" | "windows" {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (/Mac/i.test(ua)) return "macos";
  if (/Win/i.test(ua)) return "windows";
  return "linux";
}

function diagnosticsFor(path: string, source: string): Diagnostic[] {
  if (!path.endsWith(".py")) return [];
  const out: Diagnostic[] = [];
  const lines = source.split("\n");
  const importsNumpy = /^\s*import numpy as np\b/m.test(source) || /^\s*from numpy import/m.test(source);
  lines.forEach((line, idx) => {
    const abs = /(["'])(\/(?:Users|home|data)\/[^"']*)\1/.exec(line);
    if (abs && abs[2] !== undefined) {
      out.push({
        path,
        line: idx + 1,
        col: line.indexOf(abs[2]),
        rule: "cli.no-absolute-paths",
        severity: "warn",
        message: `Absolute path ${abs[2]} hard-coded in a script`,
        hint: "Take the location as a CLI option or read it from a configured data dir.",
        related: [],
      });
    }
    if (!importsNumpy && /\bnp\./.test(line) && !/^\s*#/.test(line)) {
      out.push({
        path,
        line: idx + 1,
        col: line.indexOf("np."),
        rule: "ruff:F821",
        severity: "error",
        message: "Undefined name `np`",
        hint: "Add `import numpy as np`.",
        related: [],
      });
    }
  });
  return out;
}

type Handlers = {
  [K in keyof Commands]: (args: Commands[K]["args"]) => Promise<Commands[K]["result"]> | Commands[K]["result"];
};

export class MockBackend implements Backend {
  private readonly latency: [number, number];
  private readonly timeScale: number;
  private readonly sessions = new Map<SessionId, MockSession>();
  private readonly subscribers = new Map<SessionId, Set<(e: SessionEvent) => void>>();
  private readonly terminals = new Map<TerminalId, MockTerminal>();
  private readonly layouts = new Map<SessionId, string>();
  private seq = 0;

  constructor(opts: MockOptions = {}) {
    this.latency = opts.latency ?? [50, 300];
    this.timeScale = opts.timeScale ?? 1;
    if (opts.seedSession ?? true) {
      const s = this.makeSession("/home/user/projects/decay-analysis", "local", "ollama/qwen2.5-coder:32b");
      s.info.open = false;
      s.info.kernel = "stopped";
      s.info.created_at = new Date(Date.now() - 86400_000).toISOString();
      s.info.last_opened_at = s.info.created_at;
    }
  }

  // ---- Backend ----

  async invoke<K extends keyof Commands>(cmd: K, args: Commands[K]["args"]): Promise<Commands[K]["result"]> {
    if (cmd === "terminal_write") {
      // Input is applied in call order; only the acknowledgement is delayed.
      const result = this.handlers.terminal_write(args as Commands["terminal_write"]["args"]);
      await this.delay();
      return result as Commands[K]["result"];
    }
    await this.delay();
    const handler = this.handlers[cmd] as (a: Commands[K]["args"]) => Promise<Commands[K]["result"]> | Commands[K]["result"];
    if (!handler) throw ipcError("internal", `unknown command ${String(cmd)}`);
    return handler(args);
  }

  async subscribe(session: SessionId, onEvent: (e: SessionEvent) => void): Promise<() => void> {
    await this.delay();
    this.requireSession(session);
    let set = this.subscribers.get(session);
    if (!set) {
      set = new Set();
      this.subscribers.set(session, set);
    }
    set.add(onEvent);
    return () => {
      set.delete(onEvent);
    };
  }

  // ---- helpers ----

  // Both always yield to the event loop so event ordering matches a real
  // backend even when the scale is 0.
  private delay(): Promise<void> {
    const [lo, hi] = this.latency;
    return sleep(lo + Math.random() * (hi - lo));
  }

  private stream(ms: number): Promise<void> {
    return sleep(ms * this.timeScale);
  }

  private emit(session: SessionId, event: SessionEvent): void {
    const set = this.subscribers.get(session);
    if (!set) return;
    for (const fn of set) fn(event);
  }

  private requireSession(id: SessionId): MockSession {
    const s = this.sessions.get(id);
    if (!s) throw ipcError("not_found", `session ${id} not found`);
    return s;
  }

  private requireOpen(id: SessionId): MockSession {
    const s = this.requireSession(id);
    if (!s.info.open) throw ipcError("invalid", `session ${id} is not open`);
    return s;
  }

  private requireKernel(id: SessionId): MockSession {
    const s = this.requireOpen(id);
    if (s.info.kernel !== "ready") throw ipcError("kernel_offline", `kernel is ${s.info.kernel}`);
    return s;
  }

  private summary(s: MockSession): SessionSummary {
    const { id, name, repo_abs, target, model, created_at, last_opened_at, open } = s.info;
    return { id, name, repo_abs, target, model, created_at, last_opened_at, open };
  }

  private makeSession(repo: string, target: string, model: string | null): MockSession {
    const id = `s${++this.seq}`;
    const convId = `${id}-c1`;
    const conv: MockConversation = {
      summary: { id: convId, title: "Conversation 1", model, mode: "build", created_at: nowIso(), turns: 0, busy: false },
      transcript: [],
      cancelled: false,
    };
    const t = TARGETS.find((x) => x.name === target) ?? TARGETS[0]!;
    const session: MockSession = {
      info: {
        id,
        name: basename(repo),
        repo_abs: repo,
        target: t.name,
        model,
        created_at: nowIso(),
        last_opened_at: nowIso(),
        open: true,
        policy: { ...DEFAULT_POLICY, protected: [...DEFAULT_POLICY.protected] },
        kernel: "starting",
        sandbox: t.sandbox,
        python_abs: t.kind === "local" ? `${repo}/.venv/bin/python` : `${t.remote_root}/${basename(repo)}/.venv/bin/python`,
        conversations: [conv.summary],
        active_conversation: convId,
        target_kind: t.kind,
        target_host: t.host,
      },
      files: fixtureFiles(),
      hashes: new Map(),
      runs: [],
      conversations: new Map([[convId, conv]]),
      pending: new Map(),
      layout: null,
      packages: DEFAULT_PACKAGES.map((p) => ({ ...p })),
      usage: { input: 0, output: 0 },
      counters: { turn: 0, run: 0, conv: 0, proposal: 0, tool: 0 },
    };
    this.sessions.set(id, session);
    return session;
  }

  private syncConversations(s: MockSession): void {
    s.info.conversations = [...s.conversations.values()].map((c) => ({ ...c.summary }));
    this.emit(s.info.id, { type: "session_changed", info: structuredClone(s.info) });
  }

  private startKernel(s: MockSession): void {
    s.info.kernel = "starting";
    void (async () => {
      await this.stream(120);
      this.emit(s.info.id, { type: "kernel_state", state: "starting", message: null });
      await this.stream(400);
      if (!s.info.open) return;
      s.info.kernel = "ready";
      this.emit(s.info.id, { type: "kernel_state", state: "ready", message: null });
    })();
  }

  private async hashOf(s: MockSession, path: string): Promise<string> {
    const content = s.files.get(path);
    if (content === undefined) throw ipcError("not_found", `${path} not found`);
    const cached = s.hashes.get(path);
    if (cached) return cached;
    const h = await sha256(content);
    s.hashes.set(path, h);
    return h;
  }

  private async writeFile(s: MockSession, path: string, content: string, by: "user" | "agent"): Promise<string> {
    s.files.set(path, content);
    s.hashes.delete(path);
    const h = await this.hashOf(s, path);
    this.emit(s.info.id, { type: "files_changed", paths: [path] });
    if (path.endsWith(".py")) {
      this.emit(s.info.id, { type: "index_changed" });
      // The core runs check_run after editor saves and accepted proposals.
      void (async () => {
        await this.stream(by === "user" ? 250 : 120);
        this.emit(s.info.id, { type: "diagnostics", paths: [path], diagnostics: diagnosticsFor(path, content) });
      })();
    }
    return h;
  }

  private tree(s: MockSession, root: string, depth: number): FileEntry[] {
    const prefix = root ? `${root.replace(/\/$/, "")}/` : "";
    const names = new Map<string, { kind: "file" | "dir"; size: number | null }>();
    for (const [path, content] of s.files) {
      if (!path.startsWith(prefix)) continue;
      const rest = path.slice(prefix.length);
      const slash = rest.indexOf("/");
      if (slash < 0) names.set(rest, { kind: "file", size: encoder.encode(content).length });
      else {
        const dir = rest.slice(0, slash);
        if (!names.has(dir)) names.set(dir, { kind: "dir", size: null });
      }
    }
    const entries: FileEntry[] = [];
    for (const [name, meta] of names) {
      const path = prefix + name;
      let children: FileEntry[] | null = null;
      if (meta.kind === "dir") {
        const collapsed = path === "out" || path === ".analyst";
        children = depth > 1 && !collapsed ? this.tree(s, path, depth - 1) : null;
      }
      entries.push({ path, name, kind: meta.kind, size: meta.size, children });
    }
    entries.sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "dir" ? -1 : 1));
    return entries;
  }

  private figureGroups(s: MockSession, limit?: number): FigureGroup[] {
    const groups = s.runs.filter((r) => r.figures.length > 0).map((r) => ({ run: structuredClone(r.view), figures: structuredClone(r.figures) }));
    return limit ? groups.slice(0, limit) : groups;
  }

  // ---- run engine ----

  private startRun(
    s: MockSession,
    script: string,
    args: string[],
    opts: { residuals?: boolean; sink?: (stream: "stdout" | "stderr", chunk: string) => void; terminal?: boolean } = {},
  ): { run: MockRun; done: Promise<RunView> } {
    const seq = ++s.counters.run;
    const run_id: RunId = runIdNow(seq + this.seq * 100);
    const stem = basename(script).replace(/\.py$/, "");
    const out_dir = `out/${stem}/${run_id}`;
    const run: MockRun = {
      view: {
        meta: {
          run_id,
          script,
          args: [...args],
          status: "running",
          exit_code: null,
          started_at: nowIso(),
          finished_at: null,
          out_dir,
          sandbox: s.info.sandbox ?? "none",
          tree_hash: null,
          pid: 40000 + seq,
          host: s.info.target_host ?? "localhost",
        },
        target: s.info.target,
        artifacts: [],
        stdout_tail: "",
        stderr_tail: "",
      },
      stdout: "",
      stderr: "",
      figures: [],
      cancelled: false,
      timers: [],
    };
    s.runs.unshift(run);
    s.files.set(
      `${out_dir}/run.json`,
      JSON.stringify({ run_id, script, args, started_at: run.view.meta.started_at, host: run.view.meta.host, tree_hash: null }, null, 2),
    );
    const sid = s.info.id;
    const live = !opts.terminal;
    if (live) this.emit(sid, { type: "run_started", run: structuredClone(run.view) });

    const output = (stream: "stdout" | "stderr", chunk: string) => {
      if (stream === "stdout") run.stdout += chunk;
      else run.stderr += chunk;
      run.view.stdout_tail = tail(run.stdout);
      run.view.stderr_tail = tail(run.stderr);
      if (opts.sink) opts.sink(stream, chunk);
      if (live) this.emit(sid, { type: "run_output", run_id, stream, chunk });
    };
    const artifact = (path: string, content: string, kind: Artifact["kind"], figure: FigureView | null) => {
      s.files.set(path, content);
      const art: Artifact = {
        kind,
        path,
        size: encoder.encode(content).length,
        created_at: nowIso(),
        rendered: kind === "figure" || kind === "json" || kind === "text",
      };
      if (figure?.sidecar) art.sidecar = figure.sidecar;
      run.view.artifacts.push(art);
      if (figure) run.figures.push(figure);
      if (live) this.emit(sid, { type: "run_artifact", run_id, artifact: structuredClone(art), figure: figure ? structuredClone(figure) : null });
    };
    const makeFigure = (name: string, svg: string, p: SimParams, title: string, ylabel: string): FigureView => {
      const path = `${out_dir}/figures/${name}.svg`;
      const sidecar = {
        run_id,
        script,
        args: [...args],
        title,
        axes: [{ title, xlabel: "t", ylabel }],
        created_at: nowIso(),
        width_in: 6.4,
        height_in: 4.4,
        dpi: 100,
      };
      s.files.set(`${out_dir}/figures/${name}.json`, JSON.stringify({ ...sidecar, seed: p.seed }, null, 2));
      return { run_id, script, path, name, format: "svg", sidecar, created_at: nowIso(), size: encoder.encode(svg).length };
    };

    const done = (async (): Promise<RunView> => {
      const step = async (ms: number) => {
        await this.stream(ms);
        if (run.cancelled) throw new Error("cancelled");
      };
      let exit = 0;
      try {
        await step(80);
        if (!s.files.has(script)) {
          output("stderr", `python: can't open file '${script}': [Errno 2] No such file or directory\n`);
          exit = 2;
        } else if (stem === "bad") {
          await step(150);
          output("stderr", "Traceback (most recent call last):\n");
          output("stderr", `  File "${script}", line 15, in <module>\n    main()\n`);
          output("stderr", `  File "${script}", line 10, in main\n    data = open("/Users/me/data/run3.csv").read()\n`);
          output("stderr", "FileNotFoundError: [Errno 2] No such file or directory: '/Users/me/data/run3.csv'\n");
          exit = 1;
        } else if (args.includes("--fail")) {
          const p = simParamsFromArgs(args);
          output("stdout", `Simulating n=${p.n} samples with tau=${p.tau}, amplitude=${p.amplitude}, noise=${p.noise} (seed ${p.seed})\n`);
          await step(200);
          output("stderr", "Traceback (most recent call last):\n");
          output("stderr", `  File "${script}", line 37, in <module>\n    main()\n`);
          output("stderr", `  File "${script}", line 16, in main\n    args = parser.parse_args()\n`);
          output("stderr", "SystemExit: 2\n");
          output("stderr", `${script}: error: unrecognized arguments: --fail\n`);
          exit = 1;
        } else {
          const p = simParamsFromArgs(args);
          output("stdout", `Simulating n=${p.n} samples with tau=${p.tau}, amplitude=${p.amplitude}, noise=${p.noise} (seed ${p.seed})\n`);
          await step(220);
          output("stdout", "Fitting single_exp with curve_fit...\n");
          await step(260);
          const fig = decaySvg(p);
          output("stdout", `tau_fit = ${fig.tauFit.toFixed(3)}\n`);
          await step(180);
          artifact(`${out_dir}/figures/decay-simulation.svg`, fig.svg, "figure", makeFigure("decay-simulation", fig.svg, p, "Decay simulation", "y"));
          if (opts.residuals) {
            await step(150);
            const svg = residualsSvg(p);
            artifact(`${out_dir}/figures/residuals.svg`, svg, "figure", makeFigure("residuals", svg, p, "Residuals", "y - fit"));
          }
          await step(120);
          output("stdout", `residual rms = ${fig.rms.toFixed(4)}\n`);
          artifact(
            `${out_dir}/results.json`,
            JSON.stringify({ tau_fit: fig.tauFit, rms: fig.rms, n: p.n, seed: p.seed }, null, 2),
            "json",
            null,
          );
          await step(100);
        }
      } catch {
        run.view.meta.status = "cancelled";
        run.view.meta.exit_code = null;
        run.view.meta.finished_at = nowIso();
        this.emit(sid, { type: "run_done", run: structuredClone(run.view) });
        return structuredClone(run.view);
      }
      run.view.meta.status = "done";
      run.view.meta.exit_code = exit;
      run.view.meta.finished_at = nowIso();
      if (!live) this.emit(sid, { type: "run_started", run: structuredClone(run.view) });
      this.emit(sid, { type: "run_done", run: structuredClone(run.view) });
      return structuredClone(run.view);
    })();
    return { run, done };
  }

  // ---- chat engine ----

  private async streamText(s: MockSession, conv: MockConversation, turn: TurnId, text: string): Promise<void> {
    const words = text.split(/(?<=\s)/);
    for (const w of words) {
      if (conv.cancelled) return;
      this.emit(s.info.id, { type: "agent_text", conversation: conv.summary.id, turn, delta: w });
      await this.stream(18);
    }
    conv.transcript.push({ ts: nowIso(), type: "assistant", text });
  }

  private toolCall(s: MockSession, conv: MockConversation, turn: TurnId, name: string, args: Record<string, unknown>, summary: string): Promise<void> {
    const id = `t${++s.counters.tool}`;
    const sid = s.info.id;
    this.emit(sid, { type: "agent_tool_call", conversation: conv.summary.id, turn, id, name, arguments: args });
    conv.transcript.push({ ts: nowIso(), type: "tool_call", id, name, arguments: args });
    return (async () => {
      await this.stream(220);
      this.emit(sid, { type: "agent_tool_result", conversation: conv.summary.id, turn, id, ok: true, summary });
      conv.transcript.push({ ts: nowIso(), type: "tool_result", id, ok: true, result: summary });
    })();
  }

  private waitForProposal(s: MockSession, proposal: ProposalView): Promise<ProposalDecision> {
    return new Promise((resolve) => {
      s.pending.set(proposal.id, { proposal, resolve });
      this.emit(s.info.id, { type: "proposal", proposal: structuredClone(proposal) });
    });
  }

  private finishTurn(s: MockSession, conv: MockConversation, turn: TurnId, text: string, files: string[], runs: RunId[], figures: string[]): void {
    const sid = s.info.id;
    conv.summary.turns += 1;
    conv.summary.busy = false;
    s.usage.input += 1800 + Math.round(Math.random() * 900);
    s.usage.output += 400 + Math.round(Math.random() * 300);
    const model = MODELS.find((m) => `${m.provider}/${m.id}` === (conv.summary.model ?? s.info.model));
    const window = model?.capabilities?.context_window ?? 32768;
    conv.transcript.push({ ts: nowIso(), type: "summary", text, files_changed: files, runs, figures });
    this.emit(sid, { type: "turn_done", conversation: conv.summary.id, turn, summary: { text, files_changed: files, runs, figures } });
    this.emit(sid, {
      type: "usage",
      conversation: conv.summary.id,
      input_tokens: s.usage.input,
      output_tokens: s.usage.output,
      context_window: window,
      context_used: Math.min(window, s.usage.input + s.usage.output),
    });
    this.syncConversations(s);
  }

  private async runTurn(s: MockSession, conv: MockConversation, turn: TurnId, text: string): Promise<void> {
    const sid = s.info.id;
    const cid = conv.summary.id;
    const mode: Mode = conv.summary.mode;
    this.emit(sid, { type: "turn_started", conversation: cid, turn });
    await this.stream(300);

    if (/error/i.test(text)) {
      await this.streamText(s, conv, turn, "Let me look into that. ");
      await this.stream(200);
      conv.summary.busy = false;
      this.emit(sid, { type: "turn_error", conversation: cid, turn, message: "model request failed: simulated upstream error (HTTP 502)" });
      this.syncConversations(s);
      return;
    }

    if (mode === "plan") {
      await this.toolCall(s, conv, turn, "lib.index", {}, "2 public functions in analysis: single_exp, residuals");
      await this.streamText(
        s,
        conv,
        turn,
        "Here is the plan.\n\n1. Add `double_exp` to `analysis/models.py`, reusing the `single_exp` conventions and a `Math:` section.\n2. Add a thin CLI `scripts/fit_decay.py` that loads the sample data with the existing loader and fits both models.\n3. Run it and compare residuals.\n\nSwitch to build mode and I will make the edits.",
      );
      if (conv.cancelled) return this.finishTurn(s, conv, turn, "Cancelled by user.", [], [], []);
      return this.finishTurn(s, conv, turn, "Proposed a three-step plan; no files changed.", [], [], []);
    }

    if (mode === "run") {
      await this.streamText(s, conv, turn, "Running the simulation at two decay constants and comparing the fits. ");
      const runs: RunId[] = [];
      const figures: string[] = [];
      for (const tau of ["1.0", "3.0"]) {
        if (conv.cancelled) break;
        await this.toolCall(s, conv, turn, "run.script", { script: "scripts/decay_sim.py", args: ["--tau", tau] }, `started run for --tau ${tau}`);
        const { run, done } = this.startRun(s, "scripts/decay_sim.py", ["--tau", tau]);
        conv.transcript.push({ ts: nowIso(), type: "run", run_id: run.view.meta.run_id, script: "scripts/decay_sim.py", args: ["--tau", tau], target: s.info.target });
        const view = await done;
        runs.push(view.meta.run_id);
        figures.push(...run.figures.map((f) => f.path));
        await this.streamText(s, conv, turn, `Run with tau=${tau} finished with exit ${view.meta.exit_code}. `);
      }
      await this.streamText(s, conv, turn, "\n\nBoth fits recover the input decay constant to within 2%; the residual rms tracks the noise sigma as expected.");
      return this.finishTurn(s, conv, turn, conv.cancelled ? "Cancelled by user." : "Ran decay_sim.py twice (tau 1.0 and 3.0); fits agree with inputs.", [], runs, figures);
    }

    // build mode
    await this.toolCall(s, conv, turn, "lib.index", {}, "2 public functions in analysis: single_exp, residuals");
    await this.streamText(s, conv, turn, "The library already has `single_exp` and `residuals`, so I will add a `double_exp` model next to them rather than a new module. ");
    if (conv.cancelled) return this.finishTurn(s, conv, turn, "Cancelled by user.", [], [], []);
    const path = "analysis/models.py";
    const original = s.files.get(path) ?? "";
    const insertAt = original.indexOf("\n\ndef residuals(");
    const proposed =
      insertAt >= 0 ? original.slice(0, insertAt) + DOUBLE_EXP_SNIPPET + original.slice(insertAt) : original.trimEnd() + "\n" + DOUBLE_EXP_SNIPPET;
    const proposal: ProposalView = {
      id: `p${++s.counters.proposal}`,
      conversation: cid,
      kind: "edit",
      path,
      summary: "Add double_exp(t, A, tau1, B, tau2, C) to analysis/models.py",
      original,
      proposed,
      diff: unifiedDiff(path, original, proposed),
      command: null,
      packages: null,
      message: null,
      in_scope: true,
      reason: s.info.policy.edits === "review" ? "review" : "ask",
    };
    conv.transcript.push({ ts: nowIso(), type: "proposal", id: proposal.id, kind: "edit", summary: proposal.summary, diff: proposal.diff ?? undefined });
    const decision = await this.waitForProposal(s, proposal);
    if (decision.decision === "reject") {
      await this.streamText(s, conv, turn, `Understood, leaving ${path} unchanged${decision.reason ? ` (${decision.reason})` : ""}.`);
      return this.finishTurn(s, conv, turn, "Proposal rejected; no files changed.", [], [], []);
    }
    const content = decision.content ?? proposed;
    const modified = content !== proposed;
    await this.writeFile(s, path, content, "agent");
    await this.streamText(s, conv, turn, modified ? "Applied your edited version of the change. " : "Applied the change. ");
    await this.streamText(s, conv, turn, "Running the simulation to check the fit and the residuals. ");
    await this.toolCall(s, conv, turn, "run.script", { script: "scripts/decay_sim.py", args: ["--tau", "1.2"] }, "started run");
    const { run, done } = this.startRun(s, "scripts/decay_sim.py", ["--tau", "1.2"], { residuals: true });
    conv.transcript.push({ ts: nowIso(), type: "run", run_id: run.view.meta.run_id, script: "scripts/decay_sim.py", args: ["--tau", "1.2"], target: s.info.target });
    const view = await done;
    await this.streamText(
      s,
      conv,
      turn,
      `\n\nThe run finished with exit ${view.meta.exit_code}; the residuals are flat, so a single exponential is adequate for this data and \`double_exp\` is available for the two-component runs.`,
    );
    return this.finishTurn(
      s,
      conv,
      turn,
      "Added double_exp to analysis/models.py and verified the fit with one run.",
      [path],
      [view.meta.run_id],
      run.figures.map((f) => f.path),
    );
  }

  // ---- terminals ----

  private termWrite(id: TerminalId, data: string): void {
    this.emit(this.terminals.get(id)?.session ?? "", { type: "terminal_data", terminal_id: id, data });
  }

  private prompt(id: TerminalId): void {
    const t = this.terminals.get(id);
    const s = t ? this.sessions.get(t.session) : undefined;
    this.termWrite(id, `\x1b[32m${s?.info.name ?? "analyst"}\x1b[0m$ `);
  }

  private async termExec(id: TerminalId, line: string): Promise<void> {
    const t = this.terminals.get(id);
    if (!t) return;
    const s = this.sessions.get(t.session);
    if (!s) return;
    const [cmd = "", ...rest] = line.trim().split(/\s+/);
    const w = (text: string) => this.termWrite(id, text.replace(/\n/g, "\r\n"));
    await this.stream(60);
    switch (cmd) {
      case "":
        break;
      case "ls": {
        const dir = rest[0]?.replace(/\/$/, "") ?? "";
        const entries = this.tree(s, dir === "." ? "" : dir, 1);
        if (entries.length === 0 && dir) w(`ls: cannot access '${dir}': No such file or directory\n`);
        else w(entries.map((e) => (e.kind === "dir" ? `\x1b[34m${e.name}\x1b[0m/` : e.name)).join("  ") + "\n");
        break;
      }
      case "pwd":
        w(`${s.info.repo_abs}\n`);
        break;
      case "python":
      case "python3":
        if (rest[0] === "--version" || rest[0] === "-V") w("Python 3.12.6\n");
        else if (rest[0] && rest[0].endsWith(".py")) {
          const script = rest[0];
          const { done } = this.startRun(s, script, rest.slice(1), { terminal: true, sink: (_stream, chunk) => w(chunk) });
          await done;
        } else w("Python 3.12.6 (main) [GCC 13.2.0] on linux\nInteractive mode is not available in the mock terminal.\n");
        break;
      case "cat": {
        const content = rest[0] ? s.files.get(rest[0]) : undefined;
        w(content !== undefined ? content + (content.endsWith("\n") ? "" : "\n") : `cat: ${rest[0] ?? ""}: No such file or directory\n`);
        break;
      }
      case "echo":
        w(rest.join(" ") + "\n");
        break;
      case "whoami":
        w("analyst\n");
        break;
      case "uv":
        w(`uv 0.5.4\n`);
        break;
      case "clear":
        this.termWrite(id, "\x1b[2J\x1b[H");
        break;
      case "exit":
        this.emit(t.session, { type: "terminal_exit", terminal_id: id, code: 0 });
        this.terminals.delete(id);
        return;
      default:
        w(`bash: ${cmd}: command not found\n`);
    }
    this.prompt(id);
  }

  private termInput(id: TerminalId, data: string): void {
    const t = this.terminals.get(id);
    if (!t) return;
    for (const ch of data) {
      if (t.busy) continue;
      if (ch === "\r" || ch === "\n") {
        this.termWrite(id, "\r\n");
        const line = t.line;
        t.line = "";
        t.busy = true;
        void this.termExec(id, line).finally(() => {
          t.busy = false;
        });
      } else if (ch === "\x7f" || ch === "\b") {
        if (t.line.length > 0) {
          t.line = t.line.slice(0, -1);
          this.termWrite(id, "\b \b");
        }
      } else if (ch === "\x03") {
        t.line = "";
        this.termWrite(id, "^C\r\n");
        this.prompt(id);
      } else if (ch === "\x0c") {
        this.termWrite(id, "\x1b[2J\x1b[H");
        this.prompt(id);
        this.termWrite(id, t.line);
      } else if (ch >= " ") {
        t.line += ch;
        this.termWrite(id, ch);
      }
    }
  }

  // ---- tasks ----

  private task(s: MockSession, lines: string[], onDone: () => void, ok = true): TaskId {
    const id: TaskId = `task${++this.seq}`;
    void (async () => {
      for (const line of lines) {
        await this.stream(260);
        this.emit(s.info.id, { type: "task_log", task_id: id, line });
      }
      await this.stream(200);
      onDone();
      this.emit(s.info.id, { type: "task_done", task_id: id, ok, message: ok ? null : "failed" });
    })();
    return id;
  }

  // ---- command handlers ----

  private readonly handlers: Handlers = {
    app_info: () => ({ version: "0.1.0-mock", home_abs: "/home/user", platform: platform(), dev_mode: true }),

    sessions_list: () =>
      [...this.sessions.values()]
        .map((s) => this.summary(s))
        .sort((a, b) => b.last_opened_at.localeCompare(a.last_opened_at)),

    session_create: ({ repo_abs, target, model }) => {
      if (!repo_abs || !repo_abs.startsWith("/")) throw ipcError("invalid", "repo_abs must be an absolute path");
      if (target && !TARGETS.some((t) => t.name === target)) throw ipcError("not_found", `target ${target} not configured`);
      const s = this.makeSession(repo_abs, target ?? "local", model ?? `${MODELS[0]!.provider}/${MODELS[0]!.id}`);
      this.startKernel(s);
      return structuredClone(s.info);
    },

    session_open: ({ session }) => {
      const s = this.requireSession(session);
      if (!s.info.open) {
        s.info.open = true;
        s.info.last_opened_at = nowIso();
        this.startKernel(s);
      }
      return structuredClone(s.info);
    },

    session_close: ({ session }) => {
      const s = this.requireSession(session);
      s.info.open = false;
      s.info.kernel = "stopped";
      for (const [id, t] of this.terminals) {
        if (t.session === session) {
          this.emit(session, { type: "terminal_exit", terminal_id: id, code: null });
          this.terminals.delete(id);
        }
      }
      this.emit(session, { type: "kernel_state", state: "stopped", message: null });
      return null;
    },

    session_info: ({ session }) => structuredClone(this.requireSession(session).info),

    session_rename: ({ session, name }) => {
      const s = this.requireSession(session);
      s.info.name = name;
      this.emit(session, { type: "session_changed", info: structuredClone(s.info) });
      return this.summary(s);
    },

    session_subscribe: () => {
      throw ipcError("invalid", "use Backend.subscribe()");
    },
    session_unsubscribe: () => null,

    session_set_target: ({ session, target }) => {
      const s = this.requireOpen(session);
      const t = TARGETS.find((x) => x.name === target);
      if (!t) throw ipcError("not_found", `target ${target} not configured`);
      s.info.kernel = "starting";
      this.emit(session, { type: "kernel_state", state: "starting", message: `binding to ${target}` });
      return this.task(
        s,
        t.kind === "ssh" ? [`Connecting to ${t.host}...`, "Checking uv on target...", "Syncing mirror (rsync)...", "Creating venv..."] : ["Binding local environment...", "Syncing venv..."],
        () => {
          s.info.target = t.name;
          s.info.target_kind = t.kind;
          s.info.target_host = t.host;
          s.info.sandbox = t.sandbox;
          s.info.kernel = "ready";
          this.emit(session, { type: "kernel_state", state: "ready", message: null });
          this.emit(session, { type: "session_changed", info: structuredClone(s.info) });
        },
      );
    },

    session_set_policy: ({ session, policy }) => {
      const s = this.requireSession(session);
      s.info.policy = { ...s.info.policy, ...policy };
      this.emit(session, { type: "session_changed", info: structuredClone(s.info) });
      return { ...s.info.policy };
    },

    layout_get: ({ session }) => this.layouts.get(session) ?? null,
    layout_save: ({ session, layout }) => {
      this.layouts.set(session, layout);
      return null;
    },

    fs_tree: ({ session, path, depth }) => this.tree(this.requireSession(session), path ?? "", depth ?? 1),

    fs_read: async ({ session, path }) => {
      const s = this.requireSession(session);
      const content = s.files.get(path);
      if (content === undefined) throw ipcError("not_found", `${path} not found`);
      if (/\.(png|pdf|npy|npz|parquet)$/.test(path)) throw ipcError("invalid", `${path} is binary`);
      return { path, content, hash: await this.hashOf(s, path) };
    },

    fs_read_binary: ({ session, path }) => {
      const s = this.requireSession(session);
      const content = s.files.get(path);
      if (content === undefined) throw ipcError("not_found", `${path} not found`);
      const mime = path.endsWith(".svg") ? "image/svg+xml" : path.endsWith(".json") ? "application/json" : "text/plain";
      return { mime, base64: base64Utf8(content) };
    },

    fs_write: async ({ session, path, content, expected_hash }) => {
      const s = this.requireSession(session);
      if (expected_hash !== undefined && s.files.has(path)) {
        const current = await this.hashOf(s, path);
        if (current !== expected_hash) throw ipcError("conflict", `${path} changed on disk`, { hash: current });
      }
      return { hash: await this.writeFile(s, path, content, "user") };
    },

    fs_create: ({ session, path, is_dir }) => {
      const s = this.requireSession(session);
      if (is_dir) {
        const keep = `${path.replace(/\/$/, "")}/.gitkeep`;
        if (![...s.files.keys()].some((k) => k.startsWith(`${path}/`))) s.files.set(keep, "");
        this.emit(session, { type: "files_changed", paths: [path] });
        return null;
      }
      if (s.files.has(path)) throw ipcError("conflict", `${path} exists`);
      void this.writeFile(s, path, "", "user");
      return null;
    },

    fs_delete: ({ session, path }) => {
      const s = this.requireSession(session);
      const removed: string[] = [];
      for (const key of [...s.files.keys()]) {
        if (key === path || key.startsWith(`${path}/`)) {
          s.files.delete(key);
          s.hashes.delete(key);
          removed.push(key);
        }
      }
      if (removed.length === 0) throw ipcError("not_found", `${path} not found`);
      this.emit(session, { type: "files_changed", paths: removed });
      if (removed.some((p) => p.endsWith(".py"))) this.emit(session, { type: "index_changed" });
      return null;
    },

    fs_rename: ({ session, from, to }) => {
      const s = this.requireSession(session);
      const moved: string[] = [];
      for (const key of [...s.files.keys()]) {
        if (key === from || key.startsWith(`${from}/`)) {
          const next = to + key.slice(from.length);
          const content = s.files.get(key) ?? "";
          s.files.delete(key);
          s.hashes.delete(key);
          s.files.set(next, content);
          moved.push(key, next);
        }
      }
      if (moved.length === 0) throw ipcError("not_found", `${from} not found`);
      this.emit(session, { type: "files_changed", paths: moved });
      if (moved.some((p) => p.endsWith(".py"))) this.emit(session, { type: "index_changed" });
      return null;
    },

    fs_search: ({ session, pattern, glob, max_results }) => {
      const s = this.requireSession(session);
      let re: RegExp;
      try {
        re = new RegExp(pattern, "i");
      } catch {
        throw ipcError("invalid", "bad pattern");
      }
      const globRe = glob ? new RegExp("^" + glob.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*\*/g, ".*").replace(/\*/g, "[^/]*") + "$") : null;
      const hits: Commands["fs_search"]["result"] = [];
      for (const [path, content] of s.files) {
        if (globRe && !globRe.test(path)) continue;
        content.split("\n").forEach((line, idx) => {
          const m = re.exec(line);
          if (m) hits.push({ path, line: idx + 1, col: m.index, text: line });
        });
        if (hits.length >= (max_results ?? 200)) break;
      }
      return hits.slice(0, max_results ?? 200);
    },

    index_get: ({ session }) => {
      const s = this.requireSession(session);
      const scripts = [...s.files.entries()].filter(([p]) => p.startsWith("scripts/") && p.endsWith(".py")).map(([p, c]) => parseScript(p, c));
      return { scripts, library: indexLibrary(s.files) };
    },

    check_run: ({ session, paths }) => {
      const s = this.requireKernel(session);
      const targets = paths ?? [...s.files.keys()].filter((p) => p.endsWith(".py") && (p.startsWith("scripts/") || p.startsWith("analysis/")));
      const diagnostics = targets.flatMap((p) => diagnosticsFor(p, s.files.get(p) ?? ""));
      this.emit(session, { type: "diagnostics", paths: targets, diagnostics });
      return diagnostics;
    },

    run_start: ({ session, script, args }) => {
      const s = this.requireKernel(session);
      const { run } = this.startRun(s, script, args);
      return structuredClone(run.view);
    },

    run_cancel: ({ session, run_id }) => {
      const s = this.requireSession(session);
      const run = s.runs.find((r) => r.view.meta.run_id === run_id);
      if (!run) throw ipcError("not_found", `run ${run_id} not found`);
      if (run.view.meta.status === "running") run.cancelled = true;
      return null;
    },

    runs_list: ({ session, limit }) => this.requireSession(session).runs.slice(0, limit ?? 100).map((r) => structuredClone(r.view)),

    run_get: ({ session, run_id }) => {
      const run = this.requireSession(session).runs.find((r) => r.view.meta.run_id === run_id);
      if (!run) throw ipcError("not_found", `run ${run_id} not found`);
      return { ...structuredClone(run.view), stdout: run.stdout, stderr: run.stderr };
    },

    out_list: ({ session, path, run_id }) => {
      const s = this.requireSession(session);
      const run = run_id ? s.runs.find((r) => r.view.meta.run_id === run_id) : undefined;
      const prefix = run ? run.view.meta.out_dir : (path ?? "out");
      const entries = [...s.files.entries()]
        .filter(([p]) => p.startsWith(`${prefix}/`))
        .map(([p, c]) => ({
          path: p,
          size: encoder.encode(c).length,
          kind: (/\.svg$/.test(p) ? "figure" : /\.json$/.test(p) ? "json" : /\.csv$/.test(p) ? "table" : "file") as Artifact["kind"],
          mtime: nowIso(),
        }));
      return { entries };
    },

    out_read: ({ session, path, max_bytes }) => {
      const s = this.requireSession(session);
      const content = s.files.get(path);
      if (content === undefined) throw ipcError("not_found", `${path} not found`);
      const max = max_bytes ?? 16384;
      const kind: Artifact["kind"] = /\.svg$/.test(path) ? "figure" : /\.json$/.test(path) ? "json" : /\.csv$/.test(path) ? "table" : "text";
      return { kind, preview: content.slice(0, max), truncated: content.length > max, summary: null };
    },

    figures_list: ({ session, limit }) => this.figureGroups(this.requireSession(session), limit),

    figure_data: ({ session, path }) => {
      const s = this.requireSession(session);
      const content = s.files.get(path);
      if (content === undefined) throw ipcError("not_found", `${path} not found`);
      return { mime: path.endsWith(".svg") ? "image/svg+xml" : "application/octet-stream", base64: base64Utf8(content) };
    },

    eq_extract: ({ session, path }) => {
      const s = this.requireKernel(session);
      const content = s.files.get(path);
      if (content === undefined) throw ipcError("not_found", `${path} not found`);
      return extractEquations(content);
    },

    eq_check: async ({ session, path, function: fn }) => {
      const s = this.requireKernel(session);
      const content = s.files.get(path);
      if (content === undefined) throw ipcError("not_found", `${path} not found`);
      await this.stream(400);
      const eq = extractEquations(content).find((e) => e.function === fn);
      if (!eq || eq.status !== "ok") {
        return { status: "unavailable", trials: 0, max_abs_err: null, max_rel_err: null, counterexample: null, reason: eq?.reason ?? "no equation" };
      }
      return { status: "agree", trials: 32, max_abs_err: 3.1e-12, max_rel_err: 8.4e-13, counterexample: null, reason: null };
    },

    env_packages: ({ session }) => this.requireSession(session).packages.map((p) => ({ ...p })),

    env_add: ({ session, packages }) => {
      const s = this.requireOpen(session);
      if (s.info.policy.packages === "deny") throw ipcError("policy_denied", "package changes are denied by policy");
      return this.task(s, [`uv add ${packages.join(" ")}`, `Resolved ${packages.length + 12} packages`, ...packages.map((p) => ` + ${p}==1.0.0`), "Updated uv.lock"], () => {
        for (const name of packages) if (!s.packages.some((p) => p.name === name)) s.packages.push({ name, version: "1.0.0", locked: true });
        this.emit(session, { type: "packages_changed" });
      });
    },

    env_remove: ({ session, packages }) => {
      const s = this.requireOpen(session);
      return this.task(s, [`uv remove ${packages.join(" ")}`, ...packages.map((p) => ` - ${p}`), "Updated uv.lock"], () => {
        s.packages = s.packages.filter((p) => !packages.includes(p.name));
        this.emit(session, { type: "packages_changed" });
      });
    },

    env_sync: ({ session }) => {
      const s = this.requireOpen(session);
      return this.task(s, ["uv sync", "Resolved 16 packages", "Audited 16 packages", "Environment is up to date"], () => {
        s.packages = s.packages.map((p) => ({ ...p, locked: true }));
        this.emit(session, { type: "packages_changed" });
      });
    },

    conversation_new: ({ session, title }) => {
      const s = this.requireSession(session);
      const id = `${session}-c${s.conversations.size + 1 + s.counters.conv++}`;
      const conv: MockConversation = {
        summary: { id, title: title ?? `Conversation ${s.conversations.size + 1}`, model: s.info.model, mode: "build", created_at: nowIso(), turns: 0, busy: false },
        transcript: [],
        cancelled: false,
      };
      s.conversations.set(id, conv);
      s.info.active_conversation = id;
      this.syncConversations(s);
      return { ...conv.summary };
    },

    conversation_select: ({ session, conversation }) => {
      const s = this.requireSession(session);
      if (!s.conversations.has(conversation)) throw ipcError("not_found", `conversation ${conversation} not found`);
      s.info.active_conversation = conversation;
      this.syncConversations(s);
      return null;
    },

    conversation_transcript: ({ session, conversation }) => {
      const conv = this.requireSession(session).conversations.get(conversation);
      if (!conv) throw ipcError("not_found", `conversation ${conversation} not found`);
      return structuredClone(conv.transcript);
    },

    chat_send: ({ session, conversation, text, attachments }) => {
      const s = this.requireOpen(session);
      const conv = s.conversations.get(conversation);
      if (!conv) throw ipcError("not_found", `conversation ${conversation} not found`);
      if (conv.summary.busy) throw ipcError("invalid", "conversation is busy");
      conv.summary.busy = true;
      conv.cancelled = false;
      const turn: TurnId = `${conversation}-turn${++s.counters.turn}`;
      conv.transcript.push({ ts: nowIso(), type: "user", text, attachments: attachments?.map((a) => a.path) });
      void this.runTurn(s, conv, turn, text).catch((e: unknown) => {
        conv.summary.busy = false;
        this.emit(session, { type: "turn_error", conversation, turn, message: e instanceof Error ? e.message : String(e) });
      });
      return turn;
    },

    chat_cancel: ({ session, conversation }) => {
      const s = this.requireSession(session);
      const conv = s.conversations.get(conversation);
      if (!conv) throw ipcError("not_found", `conversation ${conversation} not found`);
      conv.cancelled = true;
      for (const [id, pending] of s.pending) {
        if (pending.proposal.conversation !== conversation) continue;
        s.pending.delete(id);
        this.emit(session, { type: "proposal_resolved", proposal_id: id, decision: "reject", by: "user" });
        pending.resolve({ decision: "reject", reason: "cancelled" });
      }
      return null;
    },

    chat_set_mode: ({ session, conversation, mode }) => {
      const s = this.requireSession(session);
      const conv = s.conversations.get(conversation);
      if (!conv) throw ipcError("not_found", `conversation ${conversation} not found`);
      conv.summary.mode = mode;
      this.syncConversations(s);
      return null;
    },

    chat_set_model: ({ session, conversation, model }) => {
      const s = this.requireSession(session);
      const conv = s.conversations.get(conversation);
      if (!conv) throw ipcError("not_found", `conversation ${conversation} not found`);
      if (!MODELS.some((m) => `${m.provider}/${m.id}` === model)) throw ipcError("not_found", `model ${model} not configured`);
      conv.summary.model = model;
      s.info.model = model;
      this.syncConversations(s);
      return null;
    },

    proposal_respond: ({ session, proposal_id, decision }) => {
      const s = this.requireSession(session);
      const pending = s.pending.get(proposal_id);
      if (!pending) throw ipcError("not_found", `proposal ${proposal_id} already resolved`);
      s.pending.delete(proposal_id);
      const conv = s.conversations.get(pending.proposal.conversation);
      conv?.transcript.push({ ts: nowIso(), type: "decision", proposal_id, decision: decision.decision, by: "user" });
      this.emit(session, { type: "proposal_resolved", proposal_id, decision: decision.decision, by: "user" });
      pending.resolve(decision);
      return null;
    },

    models_list: () => structuredClone(MODELS),

    model_probe: async ({ model }) => {
      await this.stream(600);
      const m = MODELS.find((x) => `${x.provider}/${x.id}` === model || x.id === model);
      if (!m) throw ipcError("not_found", `model ${model} not configured`);
      return structuredClone(m);
    },

    targets_list: () => structuredClone(TARGETS),

    target_bootstrap: ({ target }) => {
      const t = TARGETS.find((x) => x.name === target);
      if (!t) throw ipcError("not_found", `target ${target} not configured`);
      const any = [...this.sessions.values()].find((s) => s.info.open);
      if (!any) return `task${++this.seq}`;
      return this.task(any, [`Bootstrapping ${target}...`, "Installing uv", "Done"], () => {
        t.bootstrapped = true;
      });
    },

    terminal_open: ({ session }) => {
      const s = this.requireOpen(session);
      const id: TerminalId = `term${++this.seq}`;
      this.terminals.set(id, { session, line: "", busy: false });
      void (async () => {
        await this.stream(150);
        this.termWrite(id, `Personal Analyst shell on ${s.info.target} (${s.info.repo_abs}), venv active\r\n`);
        this.prompt(id);
      })();
      return id;
    },

    terminal_write: ({ terminal_id, data }) => {
      if (!this.terminals.has(terminal_id)) throw ipcError("not_found", `terminal ${terminal_id} not found`);
      this.termInput(terminal_id, data);
      return null;
    },

    terminal_resize: ({ terminal_id }) => {
      if (!this.terminals.has(terminal_id)) throw ipcError("not_found", `terminal ${terminal_id} not found`);
      return null;
    },

    terminal_close: ({ terminal_id }) => {
      const t = this.terminals.get(terminal_id);
      if (t) {
        this.terminals.delete(terminal_id);
        this.emit(t.session, { type: "terminal_exit", terminal_id, code: 0 });
      }
      return null;
    },

    // No native picker without Tauri; the dialog falls back to a typed path.
    dialog_pick_directory: () => null,
  };
}
