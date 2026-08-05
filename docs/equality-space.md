# Exact equality space

Equality nodes are not direct MemoryIR anchors. The client explicitly selects and materializes a node through `equality.materialize`; the resulting proved CandidateForest revision can then be passed to `memory.create`. Memory planning never ranks or extracts an equality member.

`EqualityStore` is an independent persistent workspace component. Each `eqs*` space has an immutable candidate anchor, its own monotonic allocator and an `er*` revision DAG. An equality revision contains whole-program nodes, proof edges, a canonical pending worklist, status and `equality_hash`; per-request new/merged counters are returned in the expansion result.

## State model

The root must be a fully verified `EquivalentToSpec` candidate revision with no open/refuted debt and no guard or fallback. Sealed exact revisions are valid roots; speculative, rejected and guarded revisions are rejected. The anchor records `spec_revision`, candidate/revision/hash and root `impl_hash`.

Every `en*` node owns a verified typed `ImplProgram`. Nodes are hash-consed by `impl_hash`; reaching the same hash through another rewrite order reuses the node. Every `ee*` edge records source/target nodes plus a stable descriptor containing before/after hashes, production rule, structural target locator, exact side conditions and semantics/validator versions. Self edges and duplicate descriptors are suppressed.

Expansion removes the next canonical work item, enumerates the complete bounded production match set in `(rule, target)` order, applies each rewrite through the shared CandidateForest production engine, verifies the result and stages new nodes/edges/work. A failure publishes nothing and consumes no IDs. Status is:

- `open` for a new root with pending work;
- `fuel_exhausted` when caller fuel ends with pending work;
- `fixed_point` when the worklist is empty.

`equality.expand` and `equality.saturate` both require an explicit base `er*`, exact expected hash and positive fuel. The former uses the per-request expansion bound; the latter uses the saturation-fuel bound. Resuming from a fuel-exhausted revision produces the same final canonical state/hash as a one-shot run.

## Canonical identity and continuation

Equality canonical v1 uses `sha256("agentir.equality.exact.v1\0" || canonical_bytes)`. It covers the anchor, node programs and hashes, proof descriptors/digests, worklist and status. It excludes timestamps, resource policy and equality revision history, so batching does not change the final state identity. `content_hash`, `spec_hash`, `impl_hash`, `proposal_hash`, `candidate_hash`, `equality_hash` and `archive_hash` remain separate contracts.

`equality.continuation` is read-only and returns bounded deterministic matches for the next pending node. It is a legality/proof menu, not ranking. `equality.query` returns counts, status, anchor and hash. `equality.evaluate` runs a selected member through the deterministic ImplIR oracle; its result is confidence/debug information only.

## Materialization

`equality.materialize` requires an explicit member and exact equality-state hash. The core reconstructs the trusted root-to-member path, forks the anchor candidate and replays every edge through ordinary `CandidateAction::ApplyKnownRewrite`. It then checks the selected terminal `impl_hash` and records equality provenance under candidate canonical/hash v3. Equality-local operation/value IDs are never copied into CandidateForest. No member is selected implicitly and no cost model exists.
