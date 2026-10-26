// Pure merge-review computation: given the original and proposed documents
// and the set of rejected chunks, produce the file content to write.
import { Chunk } from "@codemirror/merge";
import { Text } from "@codemirror/state";

export interface MergeChunk {
  index: number;
  fromA: number;
  toA: number;
  fromB: number;
  toB: number;
}

export function listChunks(original: string, proposed: string): MergeChunk[] {
  const chunks = Chunk.build(Text.of(original.split("\n")), Text.of(proposed.split("\n")));
  return chunks.map((c, index) => ({ index, fromA: c.fromA, toA: c.toA, fromB: c.fromB, toB: c.toB }));
}

export function computeMergeResult(original: string, proposed: string, rejected: ReadonlySet<number>): string {
  const chunks = listChunks(original, proposed);
  let out = "";
  let posB = 0;
  for (const c of chunks) {
    out += proposed.slice(posB, c.fromB);
    out += rejected.has(c.index) ? original.slice(c.fromA, c.toA) : proposed.slice(c.fromB, c.toB);
    posB = c.toB;
  }
  out += proposed.slice(posB);
  return out;
}
