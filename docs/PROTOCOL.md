# Protocol and Integration Contract

*Status: phase 0 baseline, protocol version 1. Normative for `python/kernel` and `crates/*`. Changes require bumping `protocol_version` and updating both sides.*

This document fixes every boundary between components so that the Python kernel, the Rust core, the CLI, and the evaluation suite can be built independently and fit together. ARCHITECTURE.md explains why; this document says exactly what.

## 1. Kernel JSON-RPC

### 1.1 Transport and framing

- The kernel is launched as a subprocess: `<tooling_python> -m analyst_kernel --python <session_python> --root <repo_root> --runs-dir <runs_dir> [--out-dir out] [--sandbox auto|bwrap|sandbox-exec|none]`.
- Local target: the core spawns it directly. Remote target: the core spawns `ssh <host> <same command>` and uses the SSH process's stdio. The kernel never knows which.
- Framing: newline-delimited JSON, one JSON-RPC 2.0 object per line, UTF-8, on stdin (core to kernel) and stdout (kernel to core). The kernel's stderr is for logs only and is never parsed. Lines never contain embedded newlines.
- Requests carry a string or integer `id`. Notifications (no `id`) flow only from kernel to core.
- Order: the kernel may answer requests out of order (runs are concurrent). Notifications for a run are ordered relative to each other.
- First request must be `kernel.hello`. Last request is `kernel.shutdown`, after which the kernel exits 0.

### 1.2 Errors

Standard JSON-RPC errors (`-32700`, `-32600`, `-32601`, `-32602`, `-32603`) plus application codes:

| code | meaning |
|---|---|
| 1000 | run not found |
| 1001 | sandbox requested but unavailable |
| 1002 | path outside the repo root (every path parameter is relative to root and must not escape it) |
| 1003 | timeout |
| 1004 | script not found or not a Python file |
| 1005 | tool missing on target (e.g. ruff, uv) |

`error.data` is an object with at least `{"detail": string}`.

### 1.3 Methods

Types are written as TypeScript. All paths are strings relative to the repo root with forward slashes unless stated. `?` marks optional.

```ts
// ---- lifecycle ----
"kernel.hello":    { core_version: string, protocol_version: 1 }
  -> { kernel_version: string, protocol_version: 1, host: string, os: "linux"|"macos"|"other",
       sandbox: "bwrap"|"sandbox-exec"|"limits"|"none", python: string, root: string, runs_dir: string }
"kernel.shutdown": {} -> { ok: true }

// ---- environment ----
"env.info":     {} -> { python: string, version: string, venv: string|null, site_packages: string[],
                        sandbox: string, host: string, os: string, uv: string|null }
"env.packages": {} -> { packages: { name: string, version: string, locked: boolean }[] }
  // "locked" = present in uv.lock when one exists, else false

// ---- indexes (AST only, never imports user code) ----
"scripts.index": { globs?: string[] }      // default from project config, else ["scripts/**/*.py"]
  -> { scripts: ScriptInfo[] }
interface ScriptInfo {
  path: string; name: string;               // name = file stem
  description: string|null;                 // ArgumentParser(description=...) literal if any
  has_main: boolean; has_main_guard: boolean;
  options: OptionInfo[];
}
interface OptionInfo {
  flags: string[]; dest: string; type: string|null;   // "int"|"float"|"str"|"Path"|null
  default: any; help: string|null; required: boolean; choices: any[]|null; action: string|null;
}

"lib.index": { package?: string }           // default from project config, else "analysis"
  -> { functions: LibFunction[] }
interface LibFunction {
  module: string; qualname: string; path: string; line: number;
  signature: string; doc_summary: string|null; has_math: boolean;
  callers: { path: string, line: number }[];   // call sites across scripts/ and the library, by name match
}

// ---- runs ----
"run.start": { script: string, args?: string[], env?: Record<string,string>, timeout_s?: number,
               network?: boolean, tree_hash?: string|null }
  -> { run_id: string, out_dir: string }      // out_dir relative to root, e.g. "out/fit_decay/20261016-181502-3fa2c1"
"run.attach": { run_id: string, stdout_offset?: number, stderr_offset?: number }
  -> { status: RunStatus, exit_code: number|null }   // then replays run.stdout/run.stderr from offsets and re-emits run.artifact for existing artifacts, then run.done if finished
"run.status": { run_id: string } -> RunMeta
"run.cancel": { run_id: string } -> { ok: boolean }
"runs.list":  { limit?: number } -> { runs: RunMeta[] }   // newest first
type RunStatus = "running"|"done"|"cancelled"|"timeout"|"failed";
interface RunMeta {
  run_id: string; script: string; args: string[]; status: RunStatus; exit_code: number|null;
  started_at: string; finished_at: string|null; out_dir: string; sandbox: string;
  tree_hash: string|null; pid: number|null; host: string;
}
// notifications (kernel -> core)
"run.stdout":   { run_id: string, chunk: string, offset: number }   // offset = byte offset of chunk start in stdout.log
"run.stderr":   { run_id: string, chunk: string, offset: number }
"run.artifact": { run_id: string, artifact: Artifact }
"run.done":     { run_id: string, status: RunStatus, exit_code: number|null, duration_s: number }
interface Artifact {
  kind: "figure"|"table"|"array"|"json"|"text"|"file";
  path: string;                 // relative to root
  size: number; created_at: string;
  sidecar?: FigureSidecar;      // for kind == "figure" when the sidecar exists
  rendered: boolean;            // true for kinds the core pulls back from remote targets: figure, json, text
}

// ---- quick execution ----
"exec.snippet": { code: string, timeout_s?: number }
  -> { ok: boolean, stdout: string, stderr: string, value_repr: string|null }
  // persistent REPL subprocess in the session venv, sandboxed like runs; `value_repr` is repr() of the last expression

// ---- outputs on the target (never transfers data) ----
"out.list": { path?: string, run_id?: string }          // one of them; default lists out/
  -> { entries: { path: string, size: number, kind: Artifact["kind"], mtime: string }[] }
"out.read": { path: string, max_bytes?: number }         // default 16384
  -> { kind: Artifact["kind"], preview: string, truncated: boolean, summary: object|null }
  // text/csv/json: head of file. csv: summary {rows_sampled, columns}. .npy/.npz: summary {shape, dtype, min, max, mean} via numpy if importable in the session venv, else kind "file" with no preview. binary: preview "" and truncated true.

// ---- equations ----
"eq.extract": { path: string, function?: string }
  -> { equations: Equation[] }
interface Equation {
  function: string; qualname: string; line: number;
  tier: "derived"|"declared"|"latex"|"none";
  latex: string|null;           // for tier derived/declared/latex
  source_expr: string|null;     // sympy-syntax expression for derived/declared
  status: "ok"|"unsupported"|"error";
  reason: string|null;          // why derivation stopped, or parse error
}
"eq.check": { path: string, function: string, trials?: number, tol?: number, ranges?: Record<string,[number,number]> }
  -> { status: "agree"|"mismatch"|"unavailable", trials: number, max_abs_err: number|null, max_rel_err: number|null,
       counterexample: { inputs: Record<string,number>, expected: number, actual: number }|null, reason: string|null }
  // default trials 32, tol 1e-6 relative with 1e-9 absolute floor, ranges default [0.1, 10] per parameter (avoids poles at 0); the real function is called in the session venv in a sandboxed subprocess

// ---- contract and lint ----
"contract.check":   { paths?: string[] } -> { diagnostics: Diagnostic[] }     // default: all scripts and the library
"contract.similar": { threshold?: number } -> { pairs: SimilarPair[] }
"lint.run":         { paths?: string[] } -> { diagnostics: Diagnostic[] }     // ruff, mapped to the same shape; rule = "ruff:<code>"
interface Diagnostic {
  path: string; line: number; col: number; rule: string; severity: "error"|"warn";
  message: string; hint: string|null; related: { path: string, line: number }[];
}
interface SimilarPair {
  kind: "duplicate"|"near-duplicate"|"unused-twin"; score: number;
  a: { path: string, qualname: string, line: number }; b: { path: string, qualname: string, line: number };
}
```

### 1.4 Run execution contract

For `run.start` the kernel:

1. Generates `run_id = <UTC YYYYmmdd-HHMMSS>-<6 lowercase hex>`.
2. Creates `<runs_dir>/<run_id>/` with `meta.json` (RunMeta), `stdout.log`, `stderr.log`, `status` (one line: `running`, then `done:<code>`, `cancelled`, `timeout`, or `failed`), `artifacts.jsonl` (one Artifact per line, appended as discovered).
3. Creates `<root>/<out_dir>/<script stem>/<run_id>/` as the run's output directory and writes `run.json` there with `{run_id, script, args, started_at, host, tree_hash}` so a run is self-describing from the repo side too.
4. Launches `<session_python> <script> <args...>` with cwd = root, detached from the kernel process (`start_new_session=True`, stdout and stderr redirected to the log files), wrapped by the sandbox (section 1.5), with environment:

| variable | value |
|---|---|
| `ANALYST_RUN_ID` | run id |
| `ANALYST_OUT_DIR` | absolute path of the run's output directory |
| `ANALYST_ROOT` | absolute repo root |
| `MPLBACKEND` | `module://analyst_mpl` |
| `MPLCONFIGDIR` | `<runs_dir>/<run_id>/mplconfig` |
| `PYTHONPATH` | the directory containing `analyst_mpl`, prepended |
| `PYTHONUNBUFFERED` | `1` |
| `HOME`, `PATH`, `LANG`, `TMPDIR`, `VIRTUAL_ENV` | passed through; everything else dropped unless in `env` |

5. Tails the log files and the output directory (poll interval 250 ms) and emits notifications. A file under the output directory becomes an Artifact when its size has been stable for one poll; `.json` files next to a figure with the same stem are sidecars, not artifacts. Artifact kinds by extension: `.png .svg .pdf` figure; `.csv .tsv .parquet` table; `.npy .npz` array; `.json` json; `.txt .md .log` text; else file.
6. Enforces `timeout_s` (default from project config, else 600) by killing the process group and writing `timeout`.
7. Survives its own restart: `run.attach` and `runs.list` read only the run directory, never kernel memory.

The script sees a plain Python process. It needs no import from the app.

**Implementation notes (kernel, phase 0).** These refine the contract above and are what the kernel actually does:

- Each run is launched through a stdlib-only supervisor process (`python -m analyst_kernel._supervise`) started in its own session. The supervisor runs the sandbox-wrapped script, owns the timeout, and writes the final `status` line and `meta.json` itself, so bookkeeping survives a kernel restart. `RunMeta.pid` is the supervisor's pid, which is also the process-group id that `run.cancel` signals; the script's pid is in `<run_dir>/child.pid`; `<run_dir>/launch.json` records `timeout_s` and `network`.
- `PYTHONPATH` for runs is `<dir containing analyst_mpl>:<inherited PYTHONPATH>:<root>`. The trailing root is required so that `python scripts/x.py` can `import analysis`.
- `status` is `done:<code>` for any non-negative exit code, including failures of the script itself; `failed` is reserved for a process that could not be launched or died by an unexpected signal.
- `eq.extract` reports tier `derived` whenever derivation succeeds, even if a `Math:` section also exists; `eq.check` verifies the declared expression when one exists, else the derived one. A malformed `Math:` section yields `tier: "declared", status: "error"`.
- `exec.snippet` raises error 1003 on timeout and restarts the REPL on the next call. REPL and eq.check scratch directories live under `<runs_dir>/_repl`, `<runs_dir>/_eqcheck`, and `<root>/<out_dir>/_repl|_eqcheck`; `runs.list` and artifact discovery skip `_`-prefixed directories.
- `cli.no-absolute-paths` also ignores docstrings. `lib.duplicate` counts statements recursively. `lint.run` maps syntax errors, `E9xx`, and `F821`-`F823` to `error`, everything else to `warn`.
- `kernel.hello` rejects any `protocol_version` other than 1 with `-32602`. The kernel exits 0 on `kernel.shutdown` or on stdin EOF; detached runs continue.

### 1.5 Sandbox wrappers

`--sandbox auto` picks the first available: `bwrap` (Linux, `bwrap` on PATH and user namespaces usable), `sandbox-exec` (macOS), else `limits`. The chosen value is reported in `kernel.hello` and in every RunMeta. The kernel builds the wrapper; this is the required policy:

| resource | bwrap | sandbox-exec |
|---|---|---|
| read | `--ro-bind / /`, `--tmpfs $HOME` then `--ro-bind` the repo root, the venv, `~/.analyst/tooling`, and each configured data dir | `(deny default)`, `(allow file-read* (subpath "/"))`, `(deny file-read* (subpath HOME))`, then allow read of root, venv, tooling, data dirs |
| write | `--bind` the run output dir, `<root>/data/derived` if it exists, `<runs_dir>/<run_id>`, and a fresh `--tmpfs /tmp` | `(allow file-write* (subpath ...))` for the same set plus `/private/tmp` and `/private/var/folders` |
| network | `--unshare-net` unless `network: true` | `(deny network*)` unless `network: true` |
| process | `--die-with-parent --new-session --unshare-pid --proc /proc --dev /dev` | `(allow process*)`, `(allow sysctl-read)`, `(allow mach-lookup)` for the minimum needed by Python |

`limits` applies `RLIMIT_AS` (default 8 GiB), `RLIMIT_NOFILE`, and the timeout only, and the run is reported as `sandbox: "limits"`. The source tree is read-only in every real sandbox; a script that tries to write outside its output directory fails.

### 1.6 Figure capture (`analyst_mpl`)

A stdlib-only Matplotlib backend module. Behavior:

- `show()` saves every open figure, then closes them. `savefig(path)` is wrapped: the figure is saved where asked (which must be under `ANALYST_OUT_DIR` in a real sandbox) and a sidecar is written next to it.
- Figures from `show()` are named `fig-<n>.png` and `fig-<n>.svg` under `$ANALYST_OUT_DIR/figures/`, numbered from 1 in creation order; if the figure has a suptitle or its first axes has a title, a slugified title is used instead (`residuals.png`), de-duplicated with `-2`, `-3`.
- Sidecar `<stem>.json`:

```ts
interface FigureSidecar {
  run_id: string; script: string; args: string[]; title: string|null;
  axes: { title: string|null, xlabel: string|null, ylabel: string|null }[];
  created_at: string; width_in: number; height_in: number; dpi: number;
}
```

- When `ANALYST_OUT_DIR` is unset the backend falls back to `./out/figures/` relative to cwd, so terminal runs outside the app still produce files.
- The module must import only the standard library and Matplotlib itself (it subclasses the Agg backend).

## 2. Configuration files

### 2.1 Global `~/.analyst/config.toml`

```toml
[defaults]
model = "anthropic/<model-id>"         # "<provider name>/<model id>"
target = "local"

[[providers]]
name = "ollama-lab"
kind = "openai-compatible"             # openai-compatible | anthropic
base_url = "http://lab-gpu:11434/v1"
api_key_env = "OLLAMA_LAB_API_KEY"     # optional; phase 0 reads keys from env vars only

[[providers]]
name = "anthropic"
kind = "anthropic"
base_url = "https://api.anthropic.com"  # optional, this is the default for kind = anthropic
api_key_env = "ANTHROPIC_API_KEY"

[[models]]
provider = "ollama-lab"
id = "some-model"
context_window = 65536                 # optional override; otherwise probed or default 32768
tools = "native"                       # optional override: native | text-protocol | none
vision = false                         # optional override

[[targets]]
name = "lab-gpu"
kind = "ssh"                           # local | ssh
host = "lab-gpu"                       # ssh alias
remote_root = "~/analyst"
python = "3.12"
sandbox = "auto"
data_dirs = ["/data/shared"]
```

Keys are read from the named environment variable in phase 0. Keychain storage is a desktop-app feature.

### 2.2 Project `<repo>/.analyst/project.toml`

```toml
[project]
scripts = ["scripts/**/*.py"]
library = "analysis"
out_dir = "out"
data_dirs = ["data"]
timeout_s = 600

[contract]
max_helper_lines = 15
near_duplicate_threshold = 0.80
min_tokens_for_similarity = 30
math_required = []                     # globs of library modules whose public functions need a Math: section
numerical_modules = ["numpy", "scipy", "pandas", "sklearn", "jax", "torch", "xarray", "statsmodels"]
script_allowed_functions = ["main", "build_parser", "parse_args"]

[contract.severity]                    # override defaults from section 3
"lib.near-duplicate" = "warn"

[policy]
edits = "review"                       # review | auto-new-files | auto
runs = "auto"                          # auto | ask
packages = "ask"                       # ask | auto | deny
shell = "ask"                          # ask | auto | deny
network = "deny"                       # deny | ask | allow
git = "ask"
protected = []
max_steps = 40

[targets.lab-gpu]                      # per-target overrides
data_dirs = ["/data/shared/run3"]
```

Missing file means all defaults.

### 2.3 Session store `~/.analyst/sessions/<id>/`

- `session.toml`: `{ id, repo, target, model, created_at, last_tree_hash }`
- `conversations/<conv-id>.jsonl`: transcript events (section 5)
- `runs.jsonl`: RunMeta mirror of every run started through this session, with `target` added
- `artifacts/`: local copies of rendered artifacts pulled from remote targets, keyed `<run_id>/<relative path>`

Phase 0 uses files only; SQLite indexing is a desktop-app feature.

## 3. Contract rules

Rule ids are stable strings. Defaults below; `[contract.severity]` overrides.

| rule | severity | detection |
|---|---|---|
| `cli.parser-required` | error | Script has no `argparse.ArgumentParser(...)` construction. |
| `cli.help-text` | error | An `add_argument` call lacks `help=`. |
| `cli.main-function` | error | No top-level `def main(`. |
| `cli.main-guard` | error | No `if __name__ == "__main__":`. |
| `cli.option-for-every-input` | error | Inside `main` or the guard block, a string literal that looks like a path (contains `/` anywhere, or ends in a known data extension and is the first positional argument) or a numeric literal other than `0`, `1`, `-1`, `2`, `0.5` is passed as a call argument to a non-builtin function, and `add_argument(... default=...)` is not the enclosing call. A bare file name in a later position, such as `os.path.join(out_dir, "summary.json")`, is an output name and is not flagged. |
| `cli.no-absolute-paths` | error | Any string literal starting with `/`, `~`, or `[A-Za-z]:\` outside `help=` strings. |
| `lib.script-defines-logic` | error | A script defines a function not in `script_allowed_functions` unless it is private (`_` prefix), at most `max_helper_lines` long, and references no module in `numerical_modules`. |
| `lib.duplicate` | error | Two functions (anywhere in scripts or library) have equal normalized-AST fingerprints and at least 3 statements. |
| `lib.near-duplicate` | warn | Normalized token similarity (`difflib.SequenceMatcher.ratio`) at or above `near_duplicate_threshold` between two functions each with at least `min_tokens_for_similarity` tokens, excluding exact duplicates already reported. |
| `lib.unused-twin` | warn | A script defines a function whose name equals a library function's name. |
| `out.layout` | warn | A call to `savefig`, `to_csv`, `to_parquet`, `np.save`, `np.savez`, or `open(..., "w"/"wb")` whose first argument is a string literal not starting with the configured `out_dir`. |
| `seed.option` | warn | Script references `random.`, `np.random`, or `default_rng` and has no option whose dest is `seed`. |
| `math.required` | warn | A public function in a module matching `math_required` has no `Math:` docstring section. |
| `doc.docstring` | warn | A public library function has no docstring. |

**AST normalization for fingerprints:** take the function body; replace every `Name.id` with `N`, every argument name with its position, every `Constant` with its type name (`C:int`, `C:str`); keep attribute names and call structure; serialize with `ast.dump(include_attributes=False)`; fingerprint is SHA-1 of that string. **Token stream for similarity:** `tokenize` the function source, drop comments, NL, INDENT/DEDENT, replace NAME tokens that are not keywords or attribute names with `N`, NUMBER with `0`, STRING with `S`.

**`Math:` docstring section:** a line `Math:` (optionally followed by text) followed by one or more indented lines of the form `<lhs> = <expr>` in SymPy syntax, where `expr` uses the function's parameter names. The last line is the declared result. `LaTeX:` sections follow the same layout and are display-only.

**Tier A derivation:** supported body = zero or more `Assign`/`AnnAssign` to simple names, then `Return` of an expression. Supported expressions: parameters, locally assigned names, numeric constants, `+ - * / ** unary-`, calls to `exp log log10 log2 sqrt sin cos tan arcsin arccos arctan asin acos atan sinh cosh tanh abs` via bare name or `np.`/`numpy.`/`math.` attribute, `np.pi`, `math.pi`, `np.e`, `math.e`. Anything else sets `status: "unsupported"` with a reason naming the construct and line. The derived expression is substituted and passed through `sympy.simplify` with a 2-second budget, falling back to the unsimplified form.

## 4. Model provider wire mapping

Model-facing tool names must match `^[a-zA-Z0-9_-]{1,64}$`. Internal ids use dots; the mapping is `.` to `_` and `mcp.<server>.<tool>` to `mcp__<server>__<tool>`.

| internal | model-facing |
|---|---|
| `fs.read` | `fs_read` |
| `run.script` | `run_script` |
| `contract.check` | `contract_check` |

**OpenAI-compatible adapter.** `POST {base_url}/chat/completions` with `stream: true`, `stream_options: {include_usage: true}` (tolerate rejection), `tools: [{type: "function", function: {name, description, parameters}}]`, messages with `role: "tool", tool_call_id, content` for results, images as `{type: "image_url", image_url: {url: "data:image/png;base64,..."}}`. SSE lines `data: {...}` and `data: [DONE]`; accumulate `choices[0].delta.tool_calls[*]` by `index` (id and name arrive first, `function.arguments` as fragments); `finish_reason` `tool_calls` or `stop`. Model listing `GET {base_url}/models`.

**Anthropic adapter.** `POST {base_url}/v1/messages` with headers `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`; `system` as a top-level string; `tools: [{name, description, input_schema}]`; tool results as a `user` message with `[{type: "tool_result", tool_use_id, content}]`; images as `{type: "image", source: {type: "base64", media_type, data}}`. Stream events: `message_start`, `content_block_start` (`text` or `tool_use` with `id`, `name`), `content_block_delta` (`text_delta` or `input_json_delta`), `content_block_stop`, `message_delta` (`stop_reason`, `usage`), `message_stop`, `ping`, `error`.

**Internal event stream** (both adapters produce exactly this):

```ts
type ModelEvent =
  | { type: "text", delta: string }
  | { type: "tool_call", id: string, name: string, arguments: object }   // emitted once the call is complete
  | { type: "usage", input_tokens: number, output_tokens: number }
  | { type: "stop", reason: "end"|"tool_calls"|"max_tokens"|"error", message?: string }
```

**Capability probe.** A tool-calling probe sends one message with a single tool `echo_probe {token: string}` and the instruction to call it with a given token; `tools: "native"` if a tool_call event with the right token arrives, else `"text-protocol"`. Context window comes from the model info endpoint when available, else the config override, else 32768. Results are cached in `~/.analyst/capabilities.json` keyed by `provider/model`.

**Text protocol** (degraded tier): the system prompt instructs the model to emit tool calls as a fenced block starting with ` ```tool_call ` containing `{"name": ..., "arguments": {...}}`; the core parses these from text and feeds results back as user messages prefixed `tool_result <name>:`.

## 5. Transcript events

`conversations/<id>.jsonl`, one JSON object per line, `ts` is RFC 3339 UTC:

```ts
type Event =
  | { ts, type: "user", text: string, attachments?: string[] }
  | { ts, type: "assistant", text: string }
  | { ts, type: "tool_call", id: string, name: string, arguments: object }
  | { ts, type: "tool_result", id: string, ok: boolean, result: any, truncated?: boolean }
  | { ts, type: "proposal", id: string, kind: "edit"|"create"|"delete"|"package"|"shell"|"commit", summary: string, diff?: string }
  | { ts, type: "decision", proposal_id: string, decision: "accept"|"reject"|"auto", by: "user"|"policy" }
  | { ts, type: "run", run_id: string, script: string, args: string[], target: string }
  | { ts, type: "checkpoint", ref: string }
  | { ts, type: "summary", text: string, files_changed: string[], runs: string[], figures: string[] }
  | { ts, type: "usage", input_tokens: number, output_tokens: number, model: string }
```

## 6. Agent tools (phase 0 set)

Arguments and results are JSON. Every tool result is capped at 16 KiB in the model context; larger results are stored under the session's `artifacts/` and the model receives the head plus `{truncated: true, artifact: "<path>"}` which it can page with `fs.read`.

| tool | args | policy class | backed by |
|---|---|---|---|
| `fs.read` | `{path, start_line?, end_line?}` | read | local repo |
| `fs.list` | `{path?, depth?}` | read | local repo |
| `fs.search` | `{pattern, glob?, max_results?}` | read | `rg --json` locally |
| `scripts.index` | `{}` | read | kernel |
| `lib.index` | `{}` | read | kernel |
| `runs.list` / `runs.get` | `{limit?}` / `{run_id}` | read | kernel + session |
| `out.list` / `out.read` | as kernel | read | kernel |
| `eq.extract` / `eq.check` | as kernel | read | kernel |
| `contract.check` / `contract.similar` / `lint.run` | as kernel | read | kernel |
| `fig.list` | `{run_id?}` | read | session artifacts |
| `target.info` | `{}` | read | core |
| `fs.propose_edit` | `{path, edits: {old: string, new: string}[]}` or `{path, content}` | edit | core; `old` must match exactly once |
| `fs.propose_create` | `{path, content}` | edit | core |
| `fs.propose_delete` | `{path}` | edit | core |
| `run.script` | `{script, args?, timeout_s?, network?}` | run | kernel; syncs mirror first on remote targets |
| `exec.snippet` | `{code}` | run | kernel |
| `pkg.add` / `pkg.remove` / `pkg.sync` | `{packages}` / `{packages}` / `{}` | package | `uv` on target |
| `shell.run` | `{command, timeout_s?}` | shell | kernel sandbox |
| `git.status` / `git.diff` | `{}` / `{path?}` | read | local git |
| `git.commit` | `{message}` | git | local git |

MCP tools and `doc.*` tools are phase 3.

## 7. Repo sync (remote targets)

File list: `git ls-files --cached --others --exclude-standard -z` in the local repo. Push: `rsync -az --delete --files-from=<list> ./ <host>:<mirror>/` when `rsync` exists on both ends, else `tar -cf - --null -T <list> | ssh <host> 'mkdir -p <mirror> && tar -xf - -C <mirror>'` (no deletion of extraneous files in the fallback; the core warns once). `tree_hash` = SHA-256 over sorted `(path, blob sha)` of the list, passed to `run.start`. Pull of rendered artifacts: `scp -q <host>:<mirror>/<path> <local>` per artifact, or a single `tar` pipe for a whole run on `run.done`.

Remote bootstrap sequence, idempotent: `mkdir -p ~/.analyst/bin ~/.analyst/tooling`; install `uv` to `~/.analyst/bin/uv` (installer script with `UV_INSTALL_DIR`, or copy the bundled binary); `uv python install <version>`; `uv venv ~/.analyst/tooling/.venv`; `scp` the kernel wheel; `uv pip install --python ~/.analyst/tooling/.venv/bin/python <wheel>`; write `~/.analyst/tooling/VERSION`. Session venv: `cd <mirror> && ~/.analyst/bin/uv sync` (or `uv venv && uv pip install -r requirements.txt`).

## 8. Evaluation tasks

`evals/tasks/<task>/task.toml`:

```toml
[task]
name = "fit-double-exponential"
prompt = "..."
mode = "build"                     # plan | build | run
timeout_s = 900
max_steps = 40

[fixture]
dir = "fixture"                    # copied into a temp repo; `git init` + initial commit; project.toml applied

[[checks]]
kind = "contract_clean"            # no error-severity diagnostics
[[checks]]
kind = "script_exists"
path = "scripts/fit_decay.py"
[[checks]]
kind = "run_succeeds"
script = "scripts/fit_decay.py"
args = ["--help"]
[[checks]]
kind = "artifact_exists"
glob = "out/fit_decay/*/figures/*.png"
min_count = 2                      # optional, default 1
[[checks]]
kind = "eq_verified"
path = "analysis/fitting.py"
function = "double_exp"
[[checks]]
kind = "no_new_duplicates"         # contract.similar reports no duplicate pairs beyond the fixture's baseline
[[checks]]
kind = "edits_within_scope"        # every accepted edit was in the agent's declared scope
[[checks]]
kind = "file_matches"
path = "analysis/fitting.py"
regex = "def double_exp\\("
[[checks]]
kind = "no_edits"                  # for run-mode tasks: no accepted edit, create, or delete proposal
[[checks]]
kind = "runs_at_least"             # at least `count` runs of `script` were started during the task
script = "scripts/decay_sim.py"
count = 3
```

Scorecard per model: `{model, tasks: [{name, passed, checks: [{kind, passed, detail}], steps, input_tokens, output_tokens, wall_s, unwanted_edits}], summary: {pass_rate, contract_rate, eq_rate, unwanted_edit_rate, tool_error_rate}}` written to `evals/results/<model>-<timestamp>.json` and a markdown table to stdout. Graders never see the model name.
