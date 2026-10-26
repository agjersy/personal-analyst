import type { IDockviewPanelProps } from "dockview-react";
import { Suspense, lazy, useCallback, useRef, useState } from "react";
import { respondProposal } from "../../state/actions";
import { useProposal } from "../../state/chat";
import type { MergeEditorHandle } from "./MergeEditor";

const MergeEditor = lazy(() => import("./MergeEditor"));

export function ProposalReviewPanel({ params, api }: IDockviewPanelProps<{ proposalId: string }>) {
  const record = useProposal(params.proposalId);
  const handleRef = useRef<MergeEditorHandle | null>(null);
  const [remaining, setRemaining] = useState(0);
  const onHandle = useCallback((h: MergeEditorHandle | null) => {
    handleRef.current = h;
  }, []);
  const onChunksChange = useCallback((n: number) => setRemaining(n), []);

  if (!record) {
    return (
      <div className="review">
        <div className="empty">
          This proposal is no longer available.{" "}
          <button type="button" className="small" onClick={() => api.close()}>
            Close
          </button>
        </div>
      </div>
    );
  }
  const { proposal, status, session } = record;
  const pending = status === "pending";
  const original = proposal.original ?? "";
  const proposed = proposal.proposed ?? "";

  const accept = async () => {
    const content = handleRef.current?.getDoc() ?? proposed;
    await respondProposal(session, proposal.id, content === proposed ? { decision: "accept" } : { decision: "accept", content });
    api.close();
  };
  const reject = async () => {
    await respondProposal(session, proposal.id, { decision: "reject" });
    api.close();
  };

  return (
    <div className="review" data-testid="proposal-review">
      <div className="review-header">
        <span className={`badge ${proposal.kind === "create" ? "success" : "accent"}`}>{proposal.kind}</span>
        <span className="mono truncate">{proposal.path}</span>
        <span className="grow small muted truncate">{proposal.summary}</span>
        {pending ? (
          <>
            <span className="small muted">{remaining} pending chunk{remaining === 1 ? "" : "s"}</span>
            <button type="button" className="small" onClick={() => handleRef.current?.acceptAll()} title="Restore every proposed chunk">
              Reset
            </button>
            <button type="button" className="small danger" onClick={() => void reject()}>
              Reject all
            </button>
            <button type="button" className="small primary" onClick={() => void accept()} data-testid="accept-all">
              Accept all
            </button>
          </>
        ) : (
          <span className={`badge ${status === "accept" || status === "auto" ? "success" : "error"}`}>
            {status === "accept" ? "accepted" : status === "auto" ? "auto-applied" : "rejected"}
          </span>
        )}
      </div>
      {proposal.reason !== "review" ? (
        <div className="editor-banner">
          {proposal.reason === "protected"
            ? "This path is protected by policy; explicit approval is required."
            : proposal.reason === "out_of_scope"
              ? "This change is outside the conversation's scope; approval is required even in auto mode."
              : "Policy asks for confirmation of this change."}
        </div>
      ) : null}
      <Suspense fallback={<div className="empty">Loading diff…</div>}>
        <MergeEditor path={proposal.path ?? "file.txt"} original={original} proposed={proposed} readOnly={!pending} onHandle={onHandle} onChunksChange={onChunksChange} />
      </Suspense>
    </div>
  );
}
