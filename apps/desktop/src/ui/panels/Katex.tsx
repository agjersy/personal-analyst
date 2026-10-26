import katex from "katex";
import "katex/dist/katex.min.css";
import { useMemo } from "react";

export default function Katex({ latex }: { latex: string }) {
  const html = useMemo(() => {
    try {
      return katex.renderToString(latex, { displayMode: true, throwOnError: false });
    } catch {
      return "";
    }
  }, [latex]);
  if (!html) return <code>{latex}</code>;
  // KaTeX output is generated from our own LaTeX string, not user HTML.
  return <div dangerouslySetInnerHTML={{ __html: html }} />;
}
