# Stage 2B contract

Stage 2B adds a bounded speculative layer above frozen SpecIR and typed ImplIR. It accepts compiler-verified replacement fragments, preserves unproved steps as explicit proof debt, and supplies one compiler-owned guarded fallback profile.

## Implemented

- alpha-normalized `SpeculativeRewriteProposal` records with compiler-assigned proposal, ImplIR, obligation, revision and evidence IDs;
- replacement of one top-level single-result operation by an ordered pure typed fragment with an exact operand boundary and one exact-typed yield;
- explicit speculative opt-in for conditional, unknown and unsupported proposals;
- separate `proposal_hash`, ordered proof debt and a proof frontier distinct from the candidate head;
- trusted canonical-identity and production-known-rewrite recognition;
- compiler-owned `i32 div(x,x) -> 1` guarded validation with `x != 0` and an immutable proved fallback;
- candidate-level lazy guarded evaluation and fixed-seed differential refutation;
- candidate hash v2, candidate event semantics v2 and mixed v1/v2 histories;
- JSONL propose/query/translation/evaluate commands, bounded continuation escape schemas and Stage 2B resource limits;
- archive/snapshot v5 with exact v4 source decoding and explicit v4 to v5 migration.

## Trust boundary

A well-typed proposal is only executable structure. It is not evidence that the implementation remains equivalent to SpecIR. Only compiler-owned validators may create correctness evidence or a guard. Positive testing adds confidence and leaves debt open; the first deterministic counterexample marks the affected obligation refuted and rejects the candidate.

An exact seal requires every debt item proved in order. A guarded seal requires the restricted compiler certificate and a fully proved exact fallback. Open, unsupported and refuted debt blocks sealing.

## Explicitly deferred

Stage 2B has no approximate refinement, tolerances, agent certificates, general guard language, SMT, theorem prover, e-graph, saturation, population search, ranking, learned cost model, performance evidence, MemoryIR, bufferization, ScheduleIR, target lowering, GPU, LLVM or MLIR integration.
