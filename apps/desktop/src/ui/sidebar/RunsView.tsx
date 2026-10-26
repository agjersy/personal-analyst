import { useRuns } from "../../state/runs";
import { useActiveSession } from "../../state/sessions";
import { formatDuration, formatTime, shortArgs } from "../common/format";
import { showPanel } from "../workbench/controller";

export function RunsView() {
  const session = useActiveSession();
  const runs = useRuns((s) => (session ? s.runs[session] : undefined));
  const selected = useRuns((s) => (session ? s.selectedRun[session] : null));
  if (!session) return <div className="empty">No session</div>;
  const select = (run_id: string) => {
    useRuns.getState().selectRun(session, run_id);
    showPanel("runoutput");
    const group = useRuns.getState().figures[session]?.find((g) => g.run.meta.run_id === run_id);
    const first = group?.figures[0];
    if (first) useRuns.getState().setActiveFigure(session, { name: first.name, run_id });
  };
  return (
    <div className="panel">
      <div className="panel-toolbar">
        <span className="grow small muted">Runs{runs ? ` (${runs.length})` : ""}</span>
      </div>
      <div className="panel-body">
        {!runs || runs.length === 0 ? <div className="empty">No runs yet</div> : null}
        <ul className="list">
          {runs?.map((r) => {
            const status = r.meta.status === "done" && r.meta.exit_code !== 0 ? "exit" : r.meta.status;
            return (
              <li key={r.meta.run_id}>
                <button
                  type="button"
                  className={`list-item${selected === r.meta.run_id ? " selected" : ""}`}
                  onClick={() => select(r.meta.run_id)}
                  data-run={r.meta.run_id}
                >
                  <div className="grow" style={{ padding: "3px 0" }}>
                    <div className="row">
                      <span className={`dot ${r.meta.status === "running" ? "starting" : status === "done" ? "ready" : "error"}`} />
                      <span className="mono truncate">{r.meta.script.split("/").pop()}</span>
                      <span className="grow" />
                      <span className={`small status-${status}`}>
                        {r.meta.status === "done" ? `exit ${r.meta.exit_code}` : r.meta.status}
                      </span>
                    </div>
                    <div className="small muted truncate">
                      {shortArgs(r.meta.args)} · {r.target} · {formatTime(r.meta.started_at)} · {formatDuration(r.meta.started_at, r.meta.finished_at)}
                    </div>
                  </div>
                </button>
              </li>
            );
          })}
        </ul>
      </div>
    </div>
  );
}
