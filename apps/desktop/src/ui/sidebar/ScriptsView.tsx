import { useEffect, useState } from "react";
import type { OptionInfo, ScriptInfo } from "../../ipc";
import { refreshIndex, runScript } from "../../state/actions";
import { useFiles, useIndex } from "../../state/files";
import { useRuns } from "../../state/runs";
import { useActiveSession, useKernel } from "../../state/sessions";
import { notifyError } from "../../state/ui";
import { formatDuration } from "../common/format";
import { openFile, showPanel } from "../workbench/controller";

type FormValues = Record<string, string | boolean>;

function defaultsFor(script: ScriptInfo): FormValues {
  const v: FormValues = {};
  for (const o of script.options) {
    if (o.action === "store_true") v[o.dest] = o.default === true;
    else if (o.action === "store_false") v[o.dest] = o.default !== false;
    else v[o.dest] = o.default === null || o.default === undefined ? "" : String(o.default);
  }
  return v;
}

/** Builds argv from form values, omitting options left at their defaults. */
export function buildArgs(script: ScriptInfo, values: FormValues): string[] {
  const args: string[] = [];
  for (const o of script.options) {
    const flag = o.flags.find((f) => f.startsWith("--")) ?? o.flags[0];
    if (!flag) continue;
    const v = values[o.dest];
    if (o.action === "store_true" || o.action === "store_false") {
      const defaultValue = o.action === "store_true" ? o.default === true : o.default !== false;
      if (v === true && !defaultValue) args.push(flag);
      continue;
    }
    const text = typeof v === "string" ? v.trim() : "";
    const def = o.default === null || o.default === undefined ? "" : String(o.default);
    if (text === "" || text === def) {
      if (o.required && text !== "") args.push(flag, text);
      continue;
    }
    args.push(flag, text);
  }
  return args;
}

function OptionField({ option, value, onChange }: { option: OptionInfo; value: string | boolean; onChange: (v: string | boolean) => void }) {
  const id = `opt-${option.dest}`;
  if (option.action === "store_true" || option.action === "store_false") {
    return (
      <>
        <label htmlFor={id}>{option.flags[0]}</label>
        <input id={id} type="checkbox" checked={value === true} onChange={(e) => onChange(e.target.checked)} />
        {option.help ? <div className="help">{option.help}</div> : null}
      </>
    );
  }
  if (option.choices) {
    return (
      <>
        <label htmlFor={id}>{option.flags[0]}</label>
        <select id={id} value={String(value)} onChange={(e) => onChange(e.target.value)}>
          {option.choices.map((c) => (
            <option key={String(c)} value={String(c)}>
              {String(c)}
            </option>
          ))}
        </select>
        {option.help ? <div className="help">{option.help}</div> : null}
      </>
    );
  }
  const numeric = option.type === "int" || option.type === "float";
  return (
    <>
      <label htmlFor={id}>
        {option.flags[0]}
        {option.type ? <span className="subtle"> {option.type}</span> : null}
      </label>
      <input
        id={id}
        type={numeric ? "number" : "text"}
        step={option.type === "float" ? "any" : undefined}
        value={String(value)}
        placeholder={option.required ? "required" : ""}
        onChange={(e) => onChange(e.target.value)}
      />
      {option.help ? <div className="help">{option.help}</div> : null}
    </>
  );
}

function ScriptCard({ script }: { script: ScriptInfo }) {
  const session = useActiveSession();
  const kernel = useKernel(session);
  const [open, setOpen] = useState(false);
  const [values, setValues] = useState<FormValues>(() => defaultsFor(script));
  const lastRun = useRuns((s) => (session ? s.runs[session]?.find((r) => r.meta.script === script.path) : undefined));
  const ready = kernel.state === "ready";

  const run = async () => {
    if (!session) return;
    const id = await runScript(session, script.path, buildArgs(script, values));
    if (id) showPanel("runoutput");
  };

  return (
    <div className="script" data-script={script.name}>
      <button type="button" className="script-header" onClick={() => setOpen(!open)} aria-expanded={open}>
        <span className="chevron subtle">{open ? "▾" : "▸"}</span>
        <span className="mono">{script.name}</span>
        <span className="grow" />
        {lastRun ? (
          <span className={`small status-${lastRun.meta.status === "done" && lastRun.meta.exit_code !== 0 ? "exit" : lastRun.meta.status}`}>
            {lastRun.meta.status === "running" ? "running" : lastRun.meta.status === "done" ? `exit ${lastRun.meta.exit_code}` : lastRun.meta.status}
          </span>
        ) : null}
      </button>
      {open ? (
        <div className="script-body stack">
          {script.description ? <div className="small muted">{script.description}</div> : null}
          <div className="row small subtle">
            <button type="button" className="ghost small" onClick={() => void openFile(script.path)}>
              {script.path}
            </button>
            {!script.has_main_guard ? <span className="badge warning">no main guard</span> : null}
          </div>
          <div className="form-grid">
            {script.options.map((o) => (
              <OptionField key={o.dest} option={o} value={values[o.dest] ?? ""} onChange={(v) => setValues({ ...values, [o.dest]: v })} />
            ))}
          </div>
          <div className="row">
            <button type="button" className="primary" onClick={() => void run()} disabled={!ready} title={ready ? "Run" : `Kernel ${kernel.state}`}>
              Run
            </button>
            <button type="button" className="ghost small" onClick={() => setValues(defaultsFor(script))}>
              Reset
            </button>
            <span className="grow" />
            {lastRun ? (
              <span className="small muted">
                last: {formatDuration(lastRun.meta.started_at, lastRun.meta.finished_at)}
              </span>
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  );
}

export function ScriptsView() {
  const session = useActiveSession();
  const index = useIndex(session);
  const stale = useFiles((s) => (session ? (s.indexStale[session] ?? false) : false));

  useEffect(() => {
    if (session && stale) void refreshIndex(session).catch(notifyError);
  }, [session, stale]);

  if (!session) return <div className="empty">No session</div>;
  return (
    <div className="panel">
      <div className="panel-toolbar">
        <span className="grow small muted">Scripts{index ? ` (${index.scripts.length})` : ""}</span>
        <button type="button" className="small" onClick={() => void refreshIndex(session).catch(notifyError)} aria-label="Refresh index">
          ↻
        </button>
      </div>
      <div className="panel-body">
        {!index ? <div className="empty">Indexing…</div> : index.scripts.length === 0 ? <div className="empty">No scripts under scripts/</div> : null}
        {index?.scripts.map((s) => <ScriptCard key={s.path} script={s} />)}
      </div>
    </div>
  );
}
