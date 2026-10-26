import { describe, expect, it } from "vitest";
import { computeMergeResult, listChunks } from "./mergeResult";

const original = ["a", "b", "c", "d", "e", "f"].join("\n") + "\n";
const proposed = ["a", "B", "c", "d", "e", "f", "g"].join("\n") + "\n";

describe("merge review result", () => {
  it("finds one chunk per changed region", () => {
    const chunks = listChunks(original, proposed);
    expect(chunks).toHaveLength(2);
  });

  it("accepting everything yields the proposed document", () => {
    expect(computeMergeResult(original, proposed, new Set())).toBe(proposed);
  });

  it("rejecting everything yields the original document", () => {
    const all = new Set(listChunks(original, proposed).map((c) => c.index));
    expect(computeMergeResult(original, proposed, all)).toBe(original);
  });

  it("rejecting a single chunk keeps the other proposed change", () => {
    expect(computeMergeResult(original, proposed, new Set([0]))).toBe(["a", "b", "c", "d", "e", "f", "g"].join("\n") + "\n");
    expect(computeMergeResult(original, proposed, new Set([1]))).toBe(["a", "B", "c", "d", "e", "f"].join("\n") + "\n");
  });

  it("handles a create proposal (empty original)", () => {
    expect(computeMergeResult("", "new\n", new Set())).toBe("new\n");
    expect(computeMergeResult("", "new\n", new Set([0]))).toBe("");
  });
});
