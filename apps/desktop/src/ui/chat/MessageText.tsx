import type { ReactNode } from "react";

/** Minimal rendering of assistant text: fenced code blocks and `inline code`. */
export function MessageText({ text, streaming }: { text: string; streaming?: boolean }) {
  const parts: ReactNode[] = [];
  const fence = /```[\w-]*\n([\s\S]*?)(?:```|$)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let key = 0;
  while ((m = fence.exec(text))) {
    if (m.index > last) parts.push(<span key={key++}>{inline(text.slice(last, m.index))}</span>);
    parts.push(<pre key={key++}>{m[1]}</pre>);
    last = m.index + m[0].length;
  }
  if (last < text.length) parts.push(<span key={key++}>{inline(text.slice(last))}</span>);
  return (
    <div className={`msg msg-assistant${streaming ? " cursor-blink" : ""}`}>
      {parts}
    </div>
  );
}

function inline(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /`([^`\n]+)`/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let key = 0;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    out.push(<code key={key++}>{m[1]}</code>);
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}
