// IPC types are generated from crates/protocol by ts-rs (`cargo test -p analyst-protocol`).
// docs/IPC.md is the contract; types.manual.ts mirrors it and types.compat.test.ts checks
// that the generated export matches it in both directions.
export * from "./generated";
export type { TurnSummaryView as TurnSummary } from "./generated";
