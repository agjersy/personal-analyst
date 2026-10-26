// A small regex-based indexer so the mock's index follows edits. It handles
// the fixture's shapes (single-line add_argument calls, top-level defs) and is
// not a Python parser.
import type { Equation, LibFunction, OptionInfo, ScriptInfo } from "../types";

function parseLiteral(src: string): unknown {
  const s = src.trim();
  if (s === "None") return null;
  if (s === "True") return true;
  if (s === "False") return false;
  if (/^-?\d+$/.test(s)) return Number(s);
  if (/^-?\d*\.\d+(e-?\d+)?$/.test(s)) return Number(s);
  const str = /^(["'])(.*)\1$/.exec(s);
  if (str) return str[2];
  if (s.startsWith("[") && s.endsWith("]")) {
    return s
      .slice(1, -1)
      .split(",")
      .map((x) => x.trim())
      .filter(Boolean)
      .map(parseLiteral);
  }
  return s;
}

function kwarg(call: string, name: string): string | null {
  const m = new RegExp(`\\b${name}\\s*=\\s*("(?:[^"\\\\]|\\\\.)*"|'(?:[^'\\\\]|\\\\.)*'|\\[[^\\]]*\\]|[^,)]+)`).exec(call);
  return m && m[1] !== undefined ? m[1] : null;
}

export function parseScript(path: string, source: string): ScriptInfo {
  const name = path.split("/").pop()?.replace(/\.py$/, "") ?? path;
  const descMatch = /ArgumentParser\([^)]*description\s*=\s*(["'])((?:[^"'\\]|\\.)*)\1/.exec(source);
  const options: OptionInfo[] = [];
  const re = /\.add_argument\(([^\n]*)\)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(source))) {
    const call = m[1] ?? "";
    const flags = [...call.matchAll(/^\s*(["'])(-{1,2}[\w-]+)\1|,\s*(["'])(-{1,2}[\w-]+)\3/g)]
      .map((x) => x[2] ?? x[4])
      .filter((x): x is string => typeof x === "string");
    const longFlag = flags.find((f) => f.startsWith("--")) ?? flags[0] ?? "";
    const destRaw = kwarg(call, "dest");
    const dest = destRaw ? String(parseLiteral(destRaw)) : longFlag.replace(/^-+/, "").replace(/-/g, "_");
    const typeRaw = kwarg(call, "type");
    const defRaw = kwarg(call, "default");
    const helpRaw = kwarg(call, "help");
    const reqRaw = kwarg(call, "required");
    const choicesRaw = kwarg(call, "choices");
    const actionRaw = kwarg(call, "action");
    const choices = choicesRaw ? parseLiteral(choicesRaw) : null;
    options.push({
      flags,
      dest,
      type: typeRaw ? typeRaw.trim() : null,
      default: defRaw ? parseLiteral(defRaw) : null,
      help: helpRaw ? String(parseLiteral(helpRaw)) : null,
      required: reqRaw ? parseLiteral(reqRaw) === true : false,
      choices: Array.isArray(choices) ? choices : null,
      action: actionRaw ? String(parseLiteral(actionRaw)) : null,
    });
  }
  return {
    path,
    name,
    description: descMatch && descMatch[2] !== undefined ? descMatch[2] : null,
    has_main: /^def main\(/m.test(source),
    has_main_guard: /if __name__ == ["']__main__["']:/.test(source),
    options,
  };
}

interface DefInfo {
  name: string;
  line: number;
  signature: string;
  doc: string;
  body: string;
}

function findDefs(source: string): DefInfo[] {
  const lines = source.split("\n");
  const defs: DefInfo[] = [];
  for (let i = 0; i < lines.length; i++) {
    const m = /^def (\w+)\((.*)\)(?:\s*->\s*[^:]+)?:\s*$/.exec(lines[i] ?? "");
    if (!m) continue;
    let end = lines.length;
    for (let j = i + 1; j < lines.length; j++) {
      if (/^(def |class |@)/.test(lines[j] ?? "")) {
        end = j;
        break;
      }
    }
    const body = lines.slice(i + 1, end).join("\n");
    const docMatch = /^\s*"""([\s\S]*?)"""/.exec(body);
    defs.push({
      name: m[1] ?? "",
      line: i + 1,
      signature: `${m[1]}(${m[2]})`,
      doc: docMatch && docMatch[1] !== undefined ? docMatch[1] : "",
      body,
    });
  }
  return defs;
}

export function indexLibrary(files: Map<string, string>, pkg = "analysis"): LibFunction[] {
  const out: LibFunction[] = [];
  for (const [path, source] of files) {
    if (!path.startsWith(`${pkg}/`) || !path.endsWith(".py")) continue;
    const module = path.replace(/\.py$/, "").replace(/\//g, ".").replace(/\.__init__$/, "");
    for (const def of findDefs(source)) {
      if (def.name.startsWith("_")) continue;
      const callers: { path: string; line: number }[] = [];
      const callRe = new RegExp(`\\b${def.name}\\(`);
      for (const [p, s] of files) {
        if (!p.endsWith(".py")) continue;
        s.split("\n").forEach((line, idx) => {
          if (p === path && idx + 1 === def.line) return;
          if (/^\s*def /.test(line)) return;
          if (callRe.test(line)) callers.push({ path: p, line: idx + 1 });
        });
      }
      const summary = def.doc.trim().split("\n")[0]?.trim() ?? "";
      out.push({
        module,
        qualname: def.name,
        path,
        line: def.line,
        signature: def.signature,
        doc_summary: summary.length > 0 ? summary : null,
        has_math: /^\s*Math:\s*$/m.test(def.doc),
        callers,
      });
    }
  }
  return out;
}

function toLatex(expr: string): string {
  return expr
    .replace(/exp\(([^()]*)\)/g, (_m, inner: string) => `e^{${inner.replace(/\s+/g, "")}}`)
    .replace(/\btau(\d?)\b/g, (_m, d: string) => (d ? `\\tau_{${d}}` : `\\tau`))
    .replace(/\s*\*\s*/g, " \\, ")
    .replace(/\b(\w+)_(\w+)\b/g, "$1_{$2}");
}

export function extractEquations(source: string): Equation[] {
  const out: Equation[] = [];
  for (const def of findDefs(source)) {
    const math = /^\s*Math:\s*\n((?:[ \t]+\S[^\n]*\n?)+)/m.exec(def.doc);
    if (math && math[1] !== undefined) {
      const expr = math[1].trim();
      const valid = /^[\w\s=+\-*/().,^]+$/.test(expr);
      out.push({
        function: def.name,
        qualname: def.name,
        line: def.line,
        tier: "declared",
        latex: valid ? toLatex(expr) : null,
        source_expr: valid ? expr : null,
        status: valid ? "ok" : "error",
        reason: valid ? null : "could not parse the Math: section",
      });
      continue;
    }
    const ret = /^\s*return\s+(.+)$/m.exec(def.body);
    if (ret && ret[1] !== undefined && /^[\w\s=+\-*/().,]+$/.test(ret[1]) && !/\w+\(/.test(ret[1])) {
      out.push({
        function: def.name,
        qualname: def.name,
        line: def.line,
        tier: "derived",
        latex: `${def.name} = ${toLatex(ret[1].trim())}`,
        source_expr: ret[1].trim(),
        status: "ok",
        reason: null,
      });
    } else {
      out.push({
        function: def.name,
        qualname: def.name,
        line: def.line,
        tier: "none",
        latex: null,
        source_expr: null,
        status: "unsupported",
        reason: "function body is not a single arithmetic expression",
      });
    }
  }
  return out;
}
