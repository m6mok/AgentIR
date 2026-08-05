# AGENTS.md

## Project intent

AgentIR is an agent-native compiler prototype. Preserve these invariants in every change:

1. The canonical program is a typed graph, never source text.
2. SpecIR is functional and immutable after `spec.freeze`.
3. Every accepted edit is an atomic transaction against an explicit base revision.
4. Persistent IDs are compiler-assigned; transaction-local IDs start with `$`.
5. Type results are inferred by the core; no implicit casts or broadcasting.
6. Serialization and traversal order must remain deterministic.
7. Stage 1 stays transport-independent and contains no GPU/LLVM/MLIR integration.
8. `content_hash`, `spec_hash`, and `archive_hash` are distinct contracts and must never be substituted.
9. Archive v1 is immutable legacy input; new saves use v2 and old archives cross an explicit migration step.

## Where to look before changing code

Use `docs/` instead of expanding this file with broad background:

- architecture and crate boundaries: `docs/architecture.md`;
- normative Stage 1 scope and invariants: `docs/stage-1-scope.md`;
- JSONL commands and ActionIR examples: `docs/protocol.md`;
- terminology: `docs/glossary.md`;
- local build, quality checks, and benchmark harness: `docs/development.md`;
- deferred work and sequencing: `docs/roadmap.md`;
- full source specification and implementation brief: `docs/reference/`;
- architectural trade-offs: `DECISIONS.md`.

When documentation and behavior disagree, consult `docs/reference/stage-1-brief.md` first for Stage 1, then `docs/reference/agentir-spec-0.1.md`. Record intentional deviations in `DECISIONS.md`.

## Change rules

- Keep transport concerns out of `agentir-core`.
- Keep filesystem persistence in `agentir-store`; core snapshots and replay must remain I/O-free.
- Prefer `BTreeMap`/`BTreeSet` where ordering affects canonical state or output.
- Never use `unsafe` in Stage 1.
- New public types and fields need rustdoc.
- New diagnostics need a stable `ErrorCode` and structured expected/actual/details where useful.
- Rejected transactions must not consume IDs, move `head`, or mutate an older revision.
- Archive loads must verify envelope checksum, every revision hash/status, and event replay before publishing a workspace.
- Semantic canonicalization must remain independent of persistent IDs, provenance, and unreachable internal graph state while preserving interface names and ordered operands.
- Any new opcode needs verifier, canonical model, interpreter behavior, protocol coverage, and tests.
- Do not silently widen Stage 1. Put future-facing work in `docs/roadmap.md` or behind a small explicit interface.

## Required checks

Run before handing off a change:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
cargo run -p agentir-cli --bin agentir < examples/saxpy.jsonl
cargo run --release -p agentir-protocol --example baseline
```

The final SAXPY response must contain `[12.0,24.0,36.0,48.0]`.
