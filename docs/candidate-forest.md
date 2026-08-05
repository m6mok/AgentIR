# CandidateForest through Stage 2B

`CandidateForest` is independent of the SpecIR revision DAG. Every `Candidate` stores one frozen SpecIR revision and immutable `spec_hash`, a root/current `cr*` revision, an immutable candidate revision map and optional fork provenance. Candidate-only allocators issue `c*`, `cr*`, `iop*`, `iv*`, `co*`, `ev*` and `p*` IDs without changing the legacy SpecIR allocator codec.

Candidate edits always name an explicit base candidate revision. The core clones/stages the forest, validates all actions, re-verifies ImplIR, extends the proof chain, recomputes hashes/evidence and publishes atomically. A stale base, stale expected `impl_hash`, invalid target, unknown rule, unknown side condition or budget failure changes no graph/head/allocator.

States are deliberately small: `draft`, `well_typed`, `equivalent`, `speculative`, `guarded`, `sealed`, `rejected`. A sealed candidate is immutable and cannot receive evidence. Forking creates a new branch identity and preserves the selected immutable proof/debt state; recovery from a bad speculative step uses a proved ancestor. History is never physically pruned.

Known rewrite continuation entries are sorted by stable rule ID and target ID, include exact side conditions, an expected hash precondition, hard applicability and reason code, and are bounded during enumeration. Stage 2B returns this trusted space separately from one bounded speculative escape schema; it never enumerates a Cartesian product of fragments.

Proposal acceptance stores normalized provenance and appends one ordered debt item. The head is newest history; `ProofFrontier` is the last consecutive compiler-proved prefix and can lag behind it. Trusted translation validation must process debt in order. `unsupported` is unresolved, a deterministic counterexample is `refuted`, and positive tests do not close debt. See [proof-debt.md](proof-debt.md).

## Exact candidate identity

Legacy `candidate_hash` v1 uses domain `agentir.candidate.exact.v1\0` and remains byte-for-byte immutable. V2 uses `agentir.candidate.exact.v2\0` and additionally covers proposal records/IDs/hashes, proof frontier, ordered debt and statuses, translation results, guard/fallback and lifecycle state. Each revision carries its hash contract, so v1 ancestors and v2 descendants coexist. Both exclude timestamps and resource policy. Two histories may share an `impl_hash` while retaining different candidate hashes.
