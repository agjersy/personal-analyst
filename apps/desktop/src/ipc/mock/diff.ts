// Minimal unified diff for the mock's proposal cards.

export function unifiedDiff(path: string, a: string, b: string): string {
  const al = a.split("\n");
  const bl = b.split("\n");
  const n = al.length;
  const m = bl.length;
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i]![j] = al[i] === bl[j] ? dp[i + 1]![j + 1]! + 1 : Math.max(dp[i + 1]![j]!, dp[i]![j + 1]!);
    }
  }
  const ops: { tag: " " | "-" | "+"; text: string }[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (al[i] === bl[j]) {
      ops.push({ tag: " ", text: al[i] ?? "" });
      i++;
      j++;
    } else if (dp[i + 1]![j]! >= dp[i]![j + 1]!) {
      ops.push({ tag: "-", text: al[i] ?? "" });
      i++;
    } else {
      ops.push({ tag: "+", text: bl[j] ?? "" });
      j++;
    }
  }
  while (i < n) ops.push({ tag: "-", text: al[i++] ?? "" });
  while (j < m) ops.push({ tag: "+", text: bl[j++] ?? "" });

  const out: string[] = [`--- a/${path}`, `+++ b/${path}`];
  const ctx = 3;
  let k = 0;
  while (k < ops.length) {
    if (ops[k]!.tag === " ") {
      k++;
      continue;
    }
    const start = Math.max(0, k - ctx);
    let end = k;
    let last = k;
    while (end < ops.length && end - last <= ctx) {
      if (ops[end]!.tag !== " ") last = end;
      end++;
    }
    end = Math.min(ops.length, last + ctx + 1);
    let aStart = 1;
    let bStart = 1;
    for (let q = 0; q < start; q++) {
      if (ops[q]!.tag !== "+") aStart++;
      if (ops[q]!.tag !== "-") bStart++;
    }
    let aCount = 0;
    let bCount = 0;
    const body: string[] = [];
    for (let q = start; q < end; q++) {
      const op = ops[q]!;
      if (op.tag !== "+") aCount++;
      if (op.tag !== "-") bCount++;
      body.push(op.tag + op.text);
    }
    out.push(`@@ -${aStart},${aCount} +${bStart},${bCount} @@`, ...body);
    k = end;
  }
  return out.join("\n") + "\n";
}
