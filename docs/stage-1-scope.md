# Stage 1.2 contract

This document remains the frozen SpecIR-layer contract. Stage 2A builds on it without changing Stage 1 event semantics, hashes or legacy archives; see [stage-2a-scope.md](stage-2a-scope.md).

## Goal

Stage 1 tests whether an LLM-facing compiler can work through small typed transactions, partial graphs and continuation frames while one compiler core guarantees semantic legality. It is not a GPU performance milestone.

## Required invariants

1. The authoritative program is a typed graph.
2. SpecIR inputs are immutable and operations create SSA values.
3. `spec.freeze` permanently closes the specification.
4. Accepted transactions create immutable child revisions.
5. Rejected transactions change neither graph, head nor ID allocator.
6. A base revision is mandatory; stale-head writes conflict unless branching is explicit.
7. Persistent IDs come from the compiler.
8. Result types are inferred; casts and broadcasting are never implicit.
9. Regions are pure and closed over explicit captures.
10. No open hole or proof obligation may enter a frozen/deployable specification.
11. Traversal, wire output, `content_hash`, semantic canonical bytes and `spec_hash` are deterministic.
12. Arithmetic has defined reference behavior and no hidden undefined behavior.
13. Archive v1/v2/v3 is validated by its immutable codec before pure migration to v4.
14. A solver result is sound; `unknown` is neither false nor proved.
15. Legacy events replay with semantics v1 and new events record semantics v2.
16. Resource rejection is atomic and budgets do not enter program identities.

## Implemented surface

Types: `bool`, `i32`, `f32`, `index`, dense tensors with static, symbolic or compact affine dimensions.

Operations: `parameter`, `constant`, `add`, `sub`, `mul`, `div`, `fma`, `compare`, `select`, `cast`, `map`, `zip_map`, full `reduce`.

State: workspace, SpecIR, revision DAG, ActionIR, typed holes, four Stage 1 proof-obligation kinds, compact constraint facts, continuation frames, exact-state canonical JSON, semantic canonical form and CPU evaluator.

Protocol: `workspace.open`, `spec.apply`, `spec.check`, `spec.freeze`, `transaction.apply`, `program.query`, `program.evaluate`, `revision.fork`, `revision.diff`, `continuation.get`, `workspace.save`, `workspace.load`, `workspace.verify_archive` and `workspace.migrate_archive`. Local persistence commands do not add filesystem effects to SpecIR. Bounded JSONL processing preserves one response per physical line.

## Explicit non-goals

GPU code generation, LLVM/MLIR, MemoryIR/ScheduleIR, autotuning, distributed runtime, MCP server, UI, arbitrary textual syntax, SMT solving and production database/security are outside Stage 1. A separate exact ImplIR/CandidateForest now exists in Stage 2A, but it does not retroactively widen the SpecIR contract.

For the exact source requirements, see [reference/stage-1-brief.md](reference/stage-1-brief.md), which takes precedence over the broader [AgentIR 0.1 draft](reference/agentir-spec-0.1.md) for this prototype.
