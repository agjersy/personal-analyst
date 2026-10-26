import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { python } from "@codemirror/lang-python";
import { HighlightStyle, bracketMatching, foldGutter, foldKeymap, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { lintGutter, lintKeymap } from "@codemirror/lint";
import type { Diagnostic as CmDiagnostic } from "@codemirror/lint";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import type { Extension, Text } from "@codemirror/state";
import {
  EditorView,
  crosshairCursor,
  drawSelection,
  dropCursor,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  rectangularSelection,
} from "@codemirror/view";
import { tags } from "@lezer/highlight";
import type { Diagnostic } from "../../ipc";

export const highlightStyle = HighlightStyle.define([
  { tag: [tags.keyword, tags.controlKeyword, tags.operatorKeyword, tags.definitionKeyword, tags.moduleKeyword], class: "tok-keyword" },
  { tag: [tags.string, tags.special(tags.string)], class: "tok-string" },
  { tag: [tags.number, tags.bool, tags.null], class: "tok-number" },
  { tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docString], class: "tok-comment" },
  { tag: [tags.function(tags.variableName), tags.function(tags.definition(tags.variableName)), tags.definition(tags.variableName)], class: "tok-def" },
  { tag: [tags.typeName, tags.className, tags.namespace], class: "tok-type" },
]);

export function languageFor(path: string): Extension {
  if (path.endsWith(".md") || path.endsWith(".markdown")) return markdown();
  if (path.endsWith(".py")) return python();
  return [];
}

export function baseExtensions(): Extension {
  return [
    lineNumbers(),
    highlightActiveLineGutter(),
    highlightSpecialChars(),
    history(),
    foldGutter(),
    drawSelection(),
    dropCursor(),
    indentOnInput(),
    syntaxHighlighting(highlightStyle, { fallback: true }),
    bracketMatching(),
    closeBrackets(),
    autocompletion(),
    rectangularSelection(),
    crosshairCursor(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    search({ top: true }),
    lintGutter(),
    keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...foldKeymap, ...completionKeymap, ...lintKeymap, indentWithTab]),
    EditorView.lineWrapping,
  ];
}

/** Maps backend diagnostics (1-based line, 0-based col) onto document ranges. */
export function toCmDiagnostics(doc: Text, diagnostics: Diagnostic[]): CmDiagnostic[] {
  const out: CmDiagnostic[] = [];
  for (const d of diagnostics) {
    if (d.line < 1 || d.line > doc.lines) continue;
    const line = doc.line(d.line);
    const from = Math.min(line.from + Math.max(0, d.col), line.to);
    out.push({
      from,
      to: line.to > from ? line.to : from,
      severity: d.severity === "error" ? "error" : "warning",
      source: d.rule,
      message: d.hint ? `${d.message}\n${d.hint}` : d.message,
    });
  }
  return out;
}
