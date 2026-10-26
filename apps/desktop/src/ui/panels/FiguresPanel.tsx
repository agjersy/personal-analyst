import { useEffect, useMemo } from "react";
import type { FigureGroup, FigureView, RunId, SessionId } from "../../ipc";
import { ensureFigureData } from "../../state/actions";
import { useChat } from "../../state/chat";
import { useRuns } from "../../state/runs";
import type { FigureSelection } from "../../state/runs";
import { useActiveSession, useSessionInfo } from "../../state/sessions";
import { formatTime, shortArgs } from "../common/format";
import { focusChatInput } from "../workbench/controller";

const NO_GROUPS: FigureGroup[] = [];
const NO_PINS: FigureSelection[] = [];

function FigureFrame({ session, figure, label }: { session: SessionId; figure: FigureView; label: string }) {
  const data = useRuns((s) => s.figureData[figure.path]);
  useEffect(() => {
    if (!data) void ensureFigureData(session, figure.path).catch(() => undefined);
  }, [session, figure.path, data]);
  return (
    <div className="figure-frame" data-testid="figure-frame">
      {data ? <img src={data} alt={figure.sidecar?.title ?? figure.name} /> : <div className="empty">Loading…</div>}
      <div className="figure-caption truncate">
        {label} · {figure.sidecar?.script ?? figure.script} {shortArgs(figure.sidecar?.args ?? [])}
      </div>
    </div>
  );
}

export function FiguresPanel() {
  const session = useActiveSession();
  const info = useSessionInfo(session);
  const groups = useRuns((s) => (session ? (s.figures[session] ?? NO_GROUPS) : NO_GROUPS));
  const active = useRuns((s) => (session ? (s.activeFigure[session] ?? null) : null));
  const pinned = useRuns((s) => (session ? (s.pinned[session] ?? NO_PINS) : NO_PINS));
  const compare = useRuns((s) => (session ? (s.compare[session] ?? null) : null));

  // Figure names across runs, newest run first.
  const names = useMemo(() => {
    const seen: string[] = [];
    for (const g of groups) for (const f of g.figures) if (!seen.includes(f.name)) seen.push(f.name);
    return seen;
  }, [groups]);

  if (!session) return <div className="empty">No session</div>;
  if (groups.length === 0) return <div className="empty">Figures appear here as runs produce them.</div>;

  const name = active && names.includes(active.name) ? active.name : names[0];
  const history = groups.filter((g) => g.figures.some((f) => f.name === name));
  const selectedRun: RunId | null = active && history.some((g) => g.run.meta.run_id === active.run_id) ? active.run_id : (history[0]?.run.meta.run_id ?? null);
  const current = history.find((g) => g.run.meta.run_id === selectedRun);
  const figure = current?.figures.find((f) => f.name === name);
  const compareGroup = compare ? history.find((g) => g.run.meta.run_id === compare && g.run.meta.run_id !== selectedRun) : undefined;
  const compareFigure = compareGroup?.figures.find((f) => f.name === name);
  const isPinned = figure ? pinned.some((p) => p.name === figure.name && p.run_id === figure.run_id) : false;

  const select = (n: string) => {
    const latest = groups.find((g) => g.figures.some((f) => f.name === n));
    if (latest) useRuns.getState().setActiveFigure(session, { name: n, run_id: latest.run.meta.run_id });
  };
  const sendToAgent = () => {
    if (!figure || !info) return;
    useChat.getState().addAttachment(info.active_conversation, { kind: "figure", path: figure.path });
    focusChatInput();
  };

  return (
    <div className="panel" data-testid="figures-panel">
      <div className="panel-toolbar">
        <div className="figure-tabs" role="tablist">
          {names.map((n) => (
            <button key={n} type="button" role="tab" aria-selected={n === name} className={`figure-tab small${n === name ? " active" : ""}`} onClick={() => select(n)} data-testid="figure-tab">
              {n}
              {pinned.some((p) => p.name === n) ? " 📌" : ""}
            </button>
          ))}
        </div>
        <span className="grow" />
        <label className="row small muted">
          run
          <select aria-label="Figure run" value={selectedRun ?? ""} onChange={(e) => name && useRuns.getState().setActiveFigure(session, { name, run_id: e.target.value })}>
            {history.map((g, i) => (
              <option key={g.run.meta.run_id} value={g.run.meta.run_id}>
                {i === 0 ? "latest · " : ""}
                {formatTime(g.run.meta.started_at)} {shortArgs(g.run.meta.args)}
              </option>
            ))}
          </select>
        </label>
        <label className="row small muted">
          compare
          <select aria-label="Compare with run" value={compare ?? ""} onChange={(e) => useRuns.getState().setCompare(session, e.target.value || null)}>
            <option value="">none</option>
            {history
              .filter((g) => g.run.meta.run_id !== selectedRun)
              .map((g) => (
                <option key={g.run.meta.run_id} value={g.run.meta.run_id}>
                  {formatTime(g.run.meta.started_at)} {shortArgs(g.run.meta.args)}
                </option>
              ))}
          </select>
        </label>
        <button type="button" className={`small${isPinned ? " primary" : ""}`} onClick={() => figure && useRuns.getState().togglePin(session, { name: figure.name, run_id: figure.run_id })} aria-pressed={isPinned} title="Keep this run selected when new runs arrive">
          {isPinned ? "Pinned" : "Pin"}
        </button>
        <button type="button" className="small" onClick={sendToAgent} disabled={!figure}>
          Send to agent
        </button>
      </div>
      <div className="panel-body">
        <div className="figure-view">
          {figure ? <FigureFrame session={session} figure={figure} label={formatTime(figure.created_at)} /> : null}
          {compareFigure ? <FigureFrame session={session} figure={compareFigure} label={`compare · ${formatTime(compareFigure.created_at)}`} /> : null}
        </div>
      </div>
    </div>
  );
}
