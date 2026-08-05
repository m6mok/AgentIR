# CandidateForest Stage 2A

`CandidateForest` is independent of the SpecIR revision DAG. Every `Candidate` stores one frozen SpecIR revision and immutable `spec_hash`, a root/current `cr*` revision, an immutable candidate revision map and optional fork provenance. Candidate-only allocators issue `c*`, `cr*`, `iop*`, `iv*`, `co*` and `ev*` IDs without changing the legacy SpecIR allocator codec.

Candidate edits always name an explicit base candidate revision. The core clones/stages the forest, validates all actions, re-verifies ImplIR, extends the proof chain, recomputes hashes/evidence and publishes atomically. A stale base, stale expected `impl_hash`, invalid target, unknown rule, unknown side condition or budget failure changes no graph/head/allocator.

States are deliberately small: `draft`, `well_typed`, `equivalent`, `sealed`, `rejected`. A sealed candidate is immutable. Forking any candidate creates a new branch identity; a fork of a sealed revision is `draft` and editable while retaining its exact equivalence proof. History is never physically pruned in Stage 2A.

Known rewrite continuation entries are sorted by stable rule ID and target ID, include exact side conditions, an expected hash precondition, hard applicability and reason code, and are bounded during enumeration.

## Exact candidate identity

`candidate_hash` uses domain `agentir.candidate.exact.v1\0`. It includes candidate/revision IDs, SpecIR anchor, persistent ImplIR IDs and graph, state, equivalence obligation, proof chain, rewrite provenance and ordered evidence references. It excludes timestamps and resource policy. Two histories may share an `impl_hash`—for example before/after pruning nodes already excluded from semantic reachability—while retaining different `candidate_hash` values.
