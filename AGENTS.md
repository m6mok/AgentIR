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
8. `content_hash`, `spec_hash`, `impl_hash`, `proposal_hash`, `candidate_hash`, `equality_hash`, `memory_hash`, and `archive_hash` are distinct contracts and must never be substituted.
9. Archive v1/v2/v3/v4/v5/v6/v7/v8 are immutable legacy inputs; new saves use v9 and old archives cross explicit migration steps.
10. Event compiler semantics and archive format versions are independent compatibility contracts.
11. Resource limits never participate in any semantic, candidate, equality, memory, or archive hash contract.
12. ImplIR is a separate typed graph anchored to one frozen `spec_hash`.
13. Testing is confidence evidence; only trusted structural certificates prove `EquivalentToSpec`.
14. Agent proposals and testing never prove equivalence; only compiler-owned validators advance the ordered proof frontier.
15. Guarded execution is limited to compiler-owned predicates with a fully proved exact lazy fallback.
16. Stage 2B contains no MemoryIR, ScheduleIR, target lowering, approximate refinement, e-graph, ranking, or population search.
17. Stage 2C equality is a bounded positive whole-program proof graph over the shared exact rewrite engine, never an e-graph, extractor, ranker or search policy.
18. Equality nodes are hash-consed by `impl_hash`; proof edges, side conditions and explanations are compiler-owned and replay-verified.
19. MemoryIR is a separate typed graph anchored to one fully proved unconditional candidate revision and immutable `spec_hash`/`impl_hash`.
20. Bufferization, alias facts, logical lifetimes, reuse proofs and `NoOverlap` guards are compiler-owned; fresh allocation is the exact fallback.
21. Stage 3 contains no raw pointers, ScheduleIR, TargetManifest, target lowering, ranking, cost model or search policy.
22. ScheduleIR is a separate typed graph anchored to one proved MemoryIR revision and immutable TargetManifest.
23. Target capabilities, iteration domains, dependencies, remainders, schedule guards and correctness certificates are compiler-owned.
24. Stage 4 contains no backend IR, machine code, device execution, autotuning, ranking, cost model or search policy.
25. BackendIR is a separate typed graph anchored to one verified ScheduleIR revision and immutable Stage 1–4 hashes.
26. WGSL is a deterministic artifact, never canonical program input; clients cannot supply source, bindings, dispatch formulas, guards or certificates.
27. Offline validation, device execution and hardware measurements never advance the correctness frontier.
28. `backend_hash`, `artifact_hash`, `device_fingerprint_hash`, and `measurement_hash` are independent contracts and exclude interactive resource policy.
29. Stage 6A evaluation is a separate non-correctness layer; records, metrics, tokens, replay, and performance observations never advance compiler proofs.
30. `corpus_hash`, `policy_hash`, `observation_hash`, `episode_hash`, `evaluation_hash`, and evaluation `archive_hash` are independent from compiler hashes and workspace archive v1–v9.
31. Free, menu, and hybrid all use the production verifier/atomic transaction path; menu forbids arbitrary escape and hybrid escape remains bounded and untrusted.
32. Stage 6B ranking is policy-owned and has no legality, proof, success, or artifact-selection authority.
33. Choice sets, visible feature schemas, ranking policies, traces, and selections use independent hashes and deterministic compiler ordering.
34. Evaluation archive v1/v2 are immutable legacy inputs; new evaluation saves use v3 and migrate only v1→v2→v3 without invented ranking or learning records.
35. Stage 6C datasets, labels, models, training runs, inference scores, work counters and benchmarks remain non-correctness evaluation data.
36. Learned inference sees only the exact Stage 6B visible frame; labels, future outcomes, reference solutions, provider/session data and split membership are forbidden inputs.
37. Learned selection uses existing score validation, tie resolution and the production verifier/atomic transaction path; failed inference consumes no compiler IDs or state.
38. Stage 7A search exists only in `agentir-policy-eval` and explores existing production-generated menu choices in isolated evaluation engines.
39. Search objectives, plans, nodes, checkpoints, traces and results are distinct non-correctness contracts; ranking/search never advance compiler proofs or publish a live workspace.
40. Deterministic search-plan envelope fields are semantic, while runtime safety limits and wall-clock observations enter no search identity.
41. Evaluation archive v1/v2/v3 remain immutable legacy inputs; new evaluation saves use v4 and migrate only v1→v2→v3→v4 without invented search history.
42. Stage 7A results are observed recommendations or bounded-frontier records, never globally optimal claims or hardware-driven artifact selection.
43. Stage 7B measurement cohorts/objectives/recommendations live only in `agentir-policy-eval` and leave every Stage 7A and compiler contract unchanged.
44. Measured search uses only verified compiler-published records, applies hardware metrics only to terminal artifacts, and performs no hardware work during search or replay.
45. Evaluation archive v1/v2/v3/v4 are immutable legacy inputs; new evaluation saves use v5 and migrate only v1→v2→v3→v4→v5 without invented measured history.
46. Measurement indifference means equivalent under one descriptor/cohort, not proven faster, statistically significant, portable, globally optimal, or correctness evidence.
47. Stage 7C acquisition lives only in `agentir-policy-eval` and performs hardware work only after an explicit acquisition start/advance command.
48. Acquisition plans use canonical artifact-hash round robin; device, timing, validation, build, measurement and trace data remain server owned.
49. Acquisition checkpoints stop only between complete slots; no partial or sentinel measurement may be published, and uncertain external crash recovery is never silently retried.
50. Acquisition replay and evaluation archive verification perform zero hardware calls; complete results become Stage 7B cohorts only through a separate explicit handoff.
51. Evaluation archive v1–v5 are immutable legacy inputs; new evaluation saves use v6 and migrate only v1→v2→v3→v4→v5→v6 without invented acquisition history.
52. Stage 7D recovery lives only in `agentir-policy-eval`, durably prepares one canonical Stage 7C slot before hardware, and never silently reruns an indeterminate attempt.
53. Recovery is single-workspace/single-writer; it proves at-most-one accepted record per slot, not exactly-once physical hardware execution.
54. Reconciliation is server owned and zero-device: zero matches permit only a separately authorized retry, one exact compatible publication may be attached, and multiple matches remain ambiguous.
55. Recovery status, checkpoint, replay, archive verification and ordinary resume accept no executor and perform no hardware work.
56. Stage 7A/7B/7C hashes, workspace archive v9, measurement record v1, compiler hashes and proof frontiers are immutable under Stage 7D.
57. Evaluation archive v1–v6 are immutable legacy inputs; new evaluation saves use v7 and migrate only v1→v2→v3→v4→v5→v6→v7 without invented recovery history.
58. Stage 7D contains no concurrency, remote workers, multi-device pooling, automatic retry, live tuning, prediction, training, statistical claims, or new search/ranking algorithms.
59. Stage 7E lives only in `agentir-policy-eval` and composes existing Stage 7A–7D records without changing their contracts or gaining correctness authority.
60. Campaign terminal artifacts are the canonical artifact-hash-ordered distinct proved/offline-valid terminals from the frozen Stage 7A result; timing cannot preselect them.
61. Only explicit campaign `execute_prepared` may perform hardware work; status, checkpoint, resume, reconcile, cohort, recommendation, result, replay, and archive verification are zero-device.
62. Campaign recovery remains Stage 7D single-writer prepare/reconcile/explicit-retry semantics and makes no exactly-once physical execution claim.
63. Campaign recommendations never publish a live artifact and do not prove speed, portability, significance, correctness, or global optimality.
64. Evaluation archive v1–v7 are immutable legacy inputs; new evaluation saves use v8 and migrate only v1→v2→v3→v4→v5→v6→v7→v8 without invented campaign history.
65. Stage 7 closure is an offline contract gate: the deterministic multi-artifact campaign, recovery, replay, archive and full workspace checks must pass without device calls. Physical GPU execution is optional compatibility evidence, never a Stage 8 prerequisite or a substitute for the offline gate.

## Where to look before changing code

Use `docs/` instead of expanding this file with broad background:

- architecture and crate boundaries: `docs/architecture.md`;
- normative Stage 1 scope and invariants: `docs/stage-1-scope.md`;
- JSONL commands and ActionIR examples: `docs/protocol.md`;
- terminology: `docs/glossary.md`;
- local build, quality checks, and benchmark harness: `docs/development.md`;
- deferred work and sequencing: `docs/roadmap.md`;
- full source specification and implementation brief: `docs/reference/`;
- Stage 2A/2B scope, ImplIR, candidates and evidence: `docs/stage-2a-scope.md`, `docs/stage-2b-scope.md`, `docs/implir.md`, `docs/candidate-forest.md`, `docs/equivalence-and-evidence.md`;
- speculative trust boundary: `docs/speculative-rewrites.md`, `docs/proof-debt.md`, `docs/translation-validation.md`, `docs/guarded-fallback.md`;
- exact equality space and proof boundary: `docs/stage-2c-scope.md`, `docs/equality-space.md`, `docs/equality-proofs.md`;
- Stage 3 physical boundary: `docs/stage-3-scope.md`, `docs/memory-ir.md`, `docs/bufferization.md`, `docs/alias-and-lifetimes.md`, `docs/guarded-memory-reuse.md`;
- Stage 4 scheduling boundary: `docs/stage-4-scope.md`, `docs/schedule-ir.md`, `docs/target-manifest.md`, `docs/schedule-legality.md`;
- Stage 5 executable boundary: `docs/stage-5-scope.md`, `docs/backend-ir.md`, `docs/backend-lowering.md`, `docs/artifact-format.md`, `docs/artifact-correctness.md`, `docs/webgpu-runtime.md`;
- Stage 6A evaluation boundary: `docs/stage-6a-scope.md`, `docs/agent-policy-evaluation.md`, `docs/evaluation-replay.md`, `docs/evaluation-fairness.md`;
- Stage 6B ranking boundary: `docs/stage-6b-scope.md`, `docs/multi-choice-continuations.md`, `docs/ranking-policy-model.md`, `docs/ranking-transcripts.md`;
- architectural trade-offs: `DECISIONS.md`.
- Stage 7B measured search: `docs/stage-7b-scope.md`, `docs/measurement-cohorts.md`, `docs/measured-search-objectives.md`, `docs/measured-recommendations.md`, `docs/measured-search-replay.md`, `docs/measured-search-fairness.md`, `docs/evaluation-archive-v5.md`.

When documentation and behavior disagree, consult `docs/reference/stage-1-brief.md` first for Stage 1, then `docs/reference/agentir-spec-0.1.md`. Record intentional deviations in `DECISIONS.md`.

## Change rules

- Keep transport concerns out of `agentir-core`.
- Keep filesystem persistence in `agentir-store`; core snapshots and replay must remain I/O-free.
- Prefer `BTreeMap`/`BTreeSet` where ordering affects canonical state or output.
- Never use `unsafe` in Stage 1.
- New public types and fields need rustdoc.
- New diagnostics need a stable `ErrorCode` and structured expected/actual/details where useful.
- Rejected transactions must not consume IDs, move `head`, or mutate an older revision.
- Rejected candidate transactions must not consume proposal/candidate/ImplIR/obligation/evidence IDs or move a candidate head.
- Rejected memory transactions must not consume memory-local IDs or move a memory head.
- Archive loads must verify envelope checksum, every revision hash/status, and event replay before publishing a workspace.
- Semantic canonicalization must remain independent of persistent IDs, provenance, and unreachable internal graph state while preserving interface names and ordered operands.
- Any new opcode needs verifier, canonical model, interpreter behavior, protocol coverage, and tests.
- Any new known rewrite needs exact side conditions, a trusted certificate, differential/property coverage, and deterministic continuation behavior.
- Never accept agent-supplied alias/lifetime proofs, memory guards or memory certificates; unsafe reuse remains rejected.
- Never accept agent-supplied guards or correctness certificates. Unsupported validation stays unresolved; positive testing stays confidence-only.
- Do not silently widen Stage 1. Put future-facing work in `docs/roadmap.md` or behind a small explicit interface.

## Required checks

Run before handing off a change:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
cargo run -p agentir-cli --bin agentir < examples/saxpy.jsonl
cargo run -p agentir-cli --bin agentir < examples/candidate_identity.jsonl
cargo run -p agentir-cli --bin agentir < examples/candidate_rewrite.jsonl
cargo run -p agentir-cli --bin agentir < examples/speculative_open.jsonl
cargo run -p agentir-cli --bin agentir < examples/speculative_promote.jsonl
cargo run -p agentir-cli --bin agentir < examples/guarded_candidate.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_saturate.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_discharge.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_materialize.jsonl
cargo run -p agentir-cli --bin agentir < examples/memory_fresh.jsonl
cargo run -p agentir-cli --bin agentir < examples/memory_reuse.jsonl
cargo run -p agentir-cli --bin agentir < examples/memory_guarded_reuse.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_to_memory.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_serial.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_tiled.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_remainder.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_fused.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_vectorized.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_guarded_memory.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_to_schedule.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_saxpy_wgsl.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_serial.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_tiled.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_remainder.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_fused.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_vectorized.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_reuse.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_guarded_memory.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_to_artifact.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_rejected_reduce.jsonl
cargo run --release -p agentir-protocol --example baseline
cargo run -p agentir-eval -- < examples/eval_free_saxpy.jsonl
cargo run -p agentir-eval -- < examples/eval_search_start.jsonl
cargo run --release -p agentir-policy-eval --example evaluation_baseline
cargo test -p agentir-policy-eval --test stage6c
cargo test -p agentir-policy-eval --test contract_registry
cargo test -p agentir-policy-eval --test stage7a
cargo test -p agentir-policy-eval --test stage7b
cargo test -p agentir-policy-eval --test stage7c
cargo test -p agentir-policy-eval --test stage7d
cargo test -p agentir-policy-eval --test stage7e
cargo run --release -p agentir-policy-eval --example stage6c_study -- --output target/stage6c-study/run-1
cargo run --release -p agentir-policy-eval --example stage6c_study -- --output target/stage6c-study/run-2
cargo run --release -p agentir-policy-eval --example stage6c_compare -- target/stage6c-study/run-1 target/stage6c-study/run-2
cargo run --release -p agentir-policy-eval --example stage7a_study -- --output target/stage7a-study/run-1
cargo run --release -p agentir-policy-eval --example stage7a_study -- --output target/stage7a-study/run-2
cargo run --release -p agentir-policy-eval --example stage7a_compare -- target/stage7a-study/run-1 target/stage7a-study/run-2
cargo run --release -p agentir-policy-eval --example stage7c_study -- --output target/stage7c-study/run-1
cargo run --release -p agentir-policy-eval --example stage7c_study -- --output target/stage7c-study/run-2
cargo run --release -p agentir-policy-eval --example stage7c_compare -- target/stage7c-study/run-1 target/stage7c-study/run-2
cargo run --release -p agentir-policy-eval --example stage7d_study -- --output target/stage7d-study/run-1
cargo run --release -p agentir-policy-eval --example stage7d_study -- --output target/stage7d-study/run-2
cargo run --release -p agentir-policy-eval --example stage7d_compare -- target/stage7d-study/run-1 target/stage7d-study/run-2
cargo run --release -p agentir-policy-eval --example stage7e_study -- --output target/stage7e-study/run-1
cargo run --release -p agentir-policy-eval --example stage7e_study -- --output target/stage7e-study/run-2
cargo run --release -p agentir-policy-eval --example stage7e_compare -- target/stage7e-study/run-1 target/stage7e-study/run-2
```

The final SAXPY response must contain `[12.0,24.0,36.0,48.0]`.
