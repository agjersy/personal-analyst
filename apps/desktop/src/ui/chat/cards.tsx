import { useEffect, useState } from "react";
import type { RunId, SessionId, TurnSummary } from "../../ipc";
import { ensureFigureData, respondProposal } from "../../state/actions";
import { useProposal } from "../../state/chat";
import type { ChatItem } from "../../state/chat";
import { useRuns } from "../../state/runs";
import { compactJson, formatDuration, shortArgs } from "../common/format";
import { openFile, openProposal, showPanel } from "../workbench/controller";

export function ToolCard({ item }: { item: Extract<ChatItem, { kind: "tool" }> }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="card" data-testid="tool-card">
      <button type="button" className="card-header" onClick={() => setOpen(!open)} aria-expanded={open}>
        <span className="subtle">{open ? "▾" : "▸"}</span>
        <span className="mono">{item.name}</span>
        <span className="muted truncate grow">{compactJson(item.arguments, 60)}</span>
        {item.result ? <span className={`badge ${item.result.ok ? "success" : "error"}`}>{item.result.ok ? "ok" : "failed"}</span> : <span className="badge">running</span>}
      </button>
      {open ? (
        <div className="card-body stack">
          <div>
            <div className="subtle">arguments</div>
            <pre>{JSON.stringify(item.arguments, null, 2)}</pre>
          </div>
          {item.result ? (
            <div>
              <div className="subtle">result</div>
              <pre>{item.result.summary}</pre>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

export function ProposalCard({ proposalId }: { proposalId: string }) {
  const record = useProposal(proposalId);
  if (!record) return null;
  const { proposal, status, by } = record;
  const pending = status === "pending";
  const reviewable = (proposal.kind === "edit" || proposal.kind === "create") && proposal.proposed !== null;
  return (
    <div className={`card proposal`} data-testid="proposal-card">
      <div className="card-header" style={{ cursor: "default" }}>
        <span className="badge accent">{proposal.kind}</span>
        <span className="grow truncate">{proposal.summary}</span>
        {!pending ? (
          <span className={`badge ${status === "reject" ? "error" : "success"}`}>
            {status === "accept" ? "accepted" : status === "auto" ? "auto" : "rejected"}
            {by === "policy" ? " by policy" : ""}
          </span>
        ) : null}
      </div>
      <div className="card-body">
        {proposal.path ? <div className="mono">{proposal.path}</div> : null}
        {proposal.command ? <pre>$ {proposal.command}</pre> : null}
        {proposal.packages ? <div className="mono">{proposal.packages.join(", ")}</div> : null}
        {proposal.message ? <pre>{proposal.message}</pre> : null}
        {proposal.reason !== "review" ? (
          <div className="muted">
            {proposal.reason === "protected" ? "Protected path" : proposal.reason === "out_of_scope" ? "Out of the conversation's scope" : "Policy asks for approval"}
          </div>
        ) : null}
        {!reviewable && proposal.diff ? <pre>{proposal.diff}</pre> : null}
      </div>
      {pending ? (
        <div className="card-actions">
          {reviewable ? (
            <button type="button" onClick={() => openProposal(proposal.id, proposal.path ?? proposal.summary)} data-testid="review-proposal">
              Review
            </button>
          ) : null}
          <span className="grow" />
          <button type="button" className="danger" onClick={() => void respondProposal(record.session, proposal.id, { decision: "reject" })}>
            Reject
          </button>
          <button type="button" className="primary" onClick={() => void respondProposal(record.session, proposal.id, { decision: "accept" })}>
            Accept
          </button>
        </div>
      ) : null}
    </div>
  );
}

function FigureThumb({ session, path, name, run_id }: { session: SessionId; path: string; name: string; run_id: RunId }) {
  const data = useRuns((s) => s.figureData[path]);
  useEffect(() => {
    if (!data) void ensureFigureData(session, path).catch(() => undefined);
  }, [session, path, data]);
  const open = () => {
    useRuns.getState().setActiveFigure(session, { name, run_id });
    showPanel("figures");
  };
  return (
    <button type="button" className="thumb" onClick={open} title={`Open ${name} in Figures`} aria-label={`Open figure ${name}`}>
      {data ? <img src={data} alt={name} /> : <span className="subtle small">{name}</span>}
    </button>
  );
}

export function RunCard({ session, run_id }: { session: SessionId; run_id: RunId }) {
  const run = useRuns((s) => s.runs[session]?.find((r) => r.meta.run_id === run_id));
  const figures = useRuns((s) => s.figures[session]?.find((g) => g.run.meta.run_id === run_id)?.figures);
  if (!run) return null;
  const status = run.meta.status === "done" && run.meta.exit_code !== 0 ? "exit" : run.meta.status;
  const open = () => {
    useRuns.getState().selectRun(session, run_id);
    showPanel("runoutput");
  };
  return (
    <div className="card" data-testid="run-card">
      <button type="button" className="card-header" onClick={open}>
        <span className={`dot ${run.meta.status === "running" ? "starting" : status === "done" ? "ready" : "error"}`} />
        <span className="mono">{run.meta.script}</span>
        <span className="muted truncate grow">{shortArgs(run.meta.args)}</span>
        <span className={`status-${status}`}>{run.meta.status === "done" ? `exit ${run.meta.exit_code}` : run.meta.status}</span>
        <span className="subtle">{formatDuration(run.meta.started_at, run.meta.finished_at)}</span>
      </button>
      {figures && figures.length > 0 ? (
        <div className="card-body">
          <div className="thumbs">
            {figures.map((f) => (
              <FigureThumb key={f.path} session={session} path={f.path} name={f.name} run_id={run_id} />
            ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}

export function SummaryCard({ summary }: { summary: TurnSummary }) {
  return (
    <div className="card" data-testid="summary-card">
      <div className="card-header" style={{ cursor: "default" }}>
        <span className="badge success">turn done</span>
        <span className="grow">{summary.text}</span>
      </div>
      {summary.files_changed.length || summary.runs.length || summary.figures.length ? (
        <div className="card-body stack">
          {summary.files_changed.length ? (
            <div className="row" style={{ flexWrap: "wrap" }}>
              <span className="subtle">files</span>
              {summary.files_changed.map((p) => (
                <button key={p} type="button" className="ghost small mono" onClick={() => void openFile(p)}>
                  {p}
                </button>
              ))}
            </div>
          ) : null}
          {summary.runs.length ? (
            <div className="row">
              <span className="subtle">runs</span>
              <span className="mono">{summary.runs.length}</span>
            </div>
          ) : null}
          {summary.figures.length ? (
            <div className="row">
              <span className="subtle">figures</span>
              <span className="mono">{summary.figures.map((f) => f.split("/").pop()).join(", ")}</span>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

export function ErrorCard({ message }: { message: string }) {
  return (
    <div className="card error" data-testid="error-card">
      <div className="card-header" style={{ cursor: "default" }}>
        <span className="badge error">error</span>
        <span className="grow">{message}</span>
      </div>
    </div>
  );
}
