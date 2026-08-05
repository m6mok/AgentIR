# Stage 2A contract

Stage 2A adds exact candidate implementations without widening the physical compiler stack.

## Implemented

- separate SSA `ImplProgram`/operation/value/region/output model and verifier;
- deterministic identity lowering of every implemented Stage 1 opcode, including `map`, `zip_map` and `reduce` regions;
- persistent `CandidateForest`, compiler-assigned IDs, immutable candidate revision DAG, atomic transactions, fork/check/seal;
- history-independent `impl_hash` and exact history-sensitive `candidate_hash`;
- compiler-owned exact rules for unreachable pruning, fully identical cast elimination and defined scalar constant folding;
- compositional `EquivalentToSpec` certificates and minimal deterministic EvidenceIR;
- fixed-seed bounded SpecIR/ImplIR differential validation with exact f32-bit comparison and matching structured evaluation errors;
- candidate JSONL commands and bounded known-rewrite continuations;
- candidate resource budgets and archive/snapshot v4 with candidate semantics v1 replay;
- fixed-seed soundness, atomicity, corruption and mutation coverage plus candidate benchmark workloads.

## Invariants

SpecIR remains authoritative, frozen and unchanged. A candidate never changes its `spec_hash`. Rejected work consumes no persistent IDs. Unknown side conditions are not proofs. Confidence testing is not correctness proof. NumericContract participates in `impl_hash` and restricts rewrites. Resource policy participates in no semantic/exact graph hash. Candidate traversal, matching, evidence and serialization are deterministic.

## Explicitly deferred

Arbitrary or agent-proposed rewrites, speculative acceptance, approximate refinement, e-graphs/saturation, beam/population search, candidate ranking, learned cost models, hardware measurement, MemoryIR/bufferization/aliasing, ScheduleIR/tiling/fusion, TargetManifest, GPU/LLVM/MLIR lowering and artifacts are outside Stage 2A.
