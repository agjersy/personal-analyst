import { useEffect } from "react";
import { backend } from "../../ipc";
import type { RunView } from "../../ipc";
import { loadRunOutput } from "../../state/actions";
import { useRuns } from "../../state/runs";
import { useActiveSession } from "../../state/sessions";
import { notifyError } from "../../state/ui";
import { formatBytes, formatDuration, shortArgs } from "../common/format";
import { openFile, showPanel } from "../workbench/controller";

const NO_RUNS: RunView[] = [];

export function RunOutputPanel() {
  const session = useActiveSession();
  const runs = useRuns((s) => (session ? (s.runs[session] ?? NO_RUNS) : NO_RUNS));
  const selected = useRuns((s) => (session ? (s.selectedRun[session] ?? null) : null));
  const run = runs.find((r) => r.meta.run_id === selected) ?? runs[0];
  const output = useRuns((s) => (run ? s.output[run.meta.run_id] : undefined));

  useEffect(() => {
    if (session && run && !output && run.meta.status !== "running") void loadRunOutput(session, run.meta.run_id).catch(() => undefined);
  }, [session, run, output]);

  if (!session) return <div className="empty">No session</div>;
  if (!run) return <div className="empty">No runs yet. Start one from the Scripts view or the chat.</div>;
  const status = run.meta.status === "done" && run.meta.exit_code !== 0 ? "exit" : run.meta.status;
  const stdout = output?.stdout ?? run.stdout_tail;
  const stderr = output?.stderr ?? run.stderr_tail;

  const openArtifact = (path: string, kind: string) => {
    if (kind === "figure") {
      const group = useRuns.getState().figures[session]?.find((g) => g.run.meta.run_id === run.meta.run_id);
      const fig = group?.figures.find((f) => f.path === path);
      if (fig) {
        useRuns.getState().setActiveFigure(session, { name: fig.name, run_id: run.meta.run_id });
        showPanel("figures");
      }
    } else void openFile(path);
  };

  return (
    <div className="panel" data-testid="run-output">
      <div className="panel-toolbar">
        <select aria-label="Run" value={run.meta.run_id} onChange={(e) => useRuns.getState().selectRun(session, e.target.value)} style={{ maxWidth: 260 }}>
          {runs.map((r) => (
            <option key={r.meta.run_id} value={r.meta.run_id}>
              {r.meta.script.split("/").pop()} {shortArgs(r.meta.args)} · {r.meta.run_id}
            </option>
          ))}
        </select>
        <span className={`status-${status}`}>{run.meta.status === "done" ? `exit ${run.meta.exit_code}` : run.meta.status}</span>
        <span className="small muted">
          {run.target} · {run.meta.sandbox} · {formatDuration(run.meta.started_at, run.meta.finished_at)}
        </span>
        {run.meta.status === "running" ? (
          <button
            type="button"
            className="small danger"
            onClick={() =>
              backend()
                .invoke("run_cancel", { session, run_id: run.meta.run_id })
                .catch(notifyError)
            }
          >
            Cancel
          </button>
        ) : null}
        <span className="grow" />
        {run.artifacts.map((a) => (
          <button key={a.path} type="button" className="small" onClick={() => openArtifact(a.path, a.kind)} title={`${a.kind} · ${formatBytes(a.size)}`}>
            {a.kind === "figure" ? "▦ " : "▤ "}
            {a.path.split("/").pop()}
          </button>
        ))}
      </div>
      <div className="panel-body">
        <div className="run-output">
          <div className="run-stream stdout">
            <div className="run-stream-title">stdout</div>
            <pre data-testid="run-stdout">{stdout || " "}</pre>
          </div>
          <div className="run-stream stderr">
            <div className="run-stream-title">stderr</div>
            <pre data-testid="run-stderr">{stderr || " "}</pre>
          </div>
        </div>
      </div>
    </div>
  );
}
