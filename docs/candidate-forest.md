# CandidateForest through Stage 2C

Memory roots accept only fully proved unconditional exact candidate revisions. Open, unsupported, refuted, rejected, or candidate-level guarded primary revisions are rejected. Memory creation and later physical edits do not consume candidate IDs, move candidate heads, or create candidate evidence.

`CandidateForest` is independent of the SpecIR revision DAG. Every `Candidate` stores one frozen SpecIR revision and immutable `spec_hash`, a root/current `cr*` revision, an immutable candidate revision map and optional fork provenance. Candidate-only allocators issue `c*`, `cr*`, `iop*`, `iv*`, `co*`, `ev*` and `p*` IDs without changing the legacy SpecIR allocator codec.

Candidate edits always name an explicit base candidate revision. The core clones/stages the forest, validates all actions, re-verifies ImplIR, extends the proof chain, recomputes hashes/evidence and publishes atomically. A stale base, stale expected `impl_hash`, invalid target, unknown rule, unknown side condition or budget failure changes no graph/head/allocator.

States are deliberately small: `draft`, `well_typed`, `equivalent`, `speculative`, `guarded`, `sealed`, `rejected`. A sealed candidate is immutable and cannot receive evidence. Forking creates a new branch identity and preserves the selected immutable proof/debt state; recovery from a bad speculative step uses a proved ancestor. History is never physically pruned.

Known rewrite continuation entries are sorted by stable rule ID and structural target locator, include exact side conditions, an expected hash precondition, hard applicability and reason code, and are bounded during enumeration. Stage 2C uses the same production matcher and transform for CandidateForest transactions, translation recognition and equality expansion, preventing proof/execution drift.

Proposal acceptance stores normalized provenance and appends one ordered debt item. The head is newest history; `ProofFrontier` is the last consecutive compiler-proved prefix and can lag behind it. Trusted translation validation must process debt in order. `unsupported` is unresolved, a deterministic counterexample is `refuted`, and positive tests do not close debt. See [proof-debt.md](proof-debt.md).

## Exact candidate identity

Legacy `candidate_hash` v1 uses domain `agentir.candidate.exact.v1\0` and remains byte-for-byte immutable. V2 uses `agentir.candidate.exact.v2\0` and additionally covers proposal records/IDs/hashes, proof frontier, ordered debt and statuses, translation results, guard/fallback and lifecycle state. V3 uses `agentir.candidate.exact.v3\0` and additionally covers compiler-built equality membership proofs and explicit materialization provenance. Each revision carries its hash contract, so v1/v2/v3 ancestors coexist. All exclude timestamps and resource policy. Two histories may share an `impl_hash` while retaining different candidate hashes.

Equality exploration does not mutate a candidate head. Materialization explicitly forks the anchor and applies the explained production path through normal atomic candidate transactions; debt discharge creates a normal candidate revision with correctness evidence. See [equality-space.md](equality-space.md) and [equality-proofs.md](equality-proofs.md).
