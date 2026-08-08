# Glossary

**MemoryIR** — separate typed graph that materializes reachable ImplIR tensor values into abstract buffer regions without changing computation semantics.

**Memory plan/revision** — independent `mp*` branch and immutable `mr*` physical-state revision anchored to one `spec_hash` and `impl_hash`.

**MemoryEquivalentToImpl** — compiler-proved relation that storage, accesses, reuse and guarded fallback preserve the anchored ImplIR interface, numeric contract and outputs.

**memory_hash** — exact history-sensitive identity of one typed MemoryIR revision, distinct from all SpecIR, candidate, equality and archive hashes.

- **ActionIR** — typed algebra of graph edits submitted by an agent.
- **Archive** — checksummed versioned workspace encoding; v1/v2/v3/v4/v5/v6 are immutable legacy inputs and v7 is current.
- **Archive hash** — version-specific integrity hash of a concrete archive body.
- **Canonical state** — deterministic serialized `Program` used for the history-sensitive `content_hash` and replay.
- **Compiler core** — transport-independent verifier and workspace state machine.
- **Compiler semantics version** — event-level selector for historical transaction inference and obligation behavior; independent of archive format.
- **Candidate semantics version** — independent selector for CandidateForest event replay; legacy exact history uses v1, Stage 2B proposal/validation uses v2 and equality-linked revisions use v3.
- **Candidate** — one persistent ImplIR branch anchored to an immutable frozen `spec_hash`.
- **Candidate hash** — per-revision v1/v2/v3 history-sensitive exact identity including IDs, proof state and evidence references.
- **CandidateForest** — independent collection of immutable candidate revision DAGs, EvidenceIR and candidate allocator/event state.
- **ConstraintFacts** — deterministic derived equality/static-binding model used to query and discharge shape relations.
- **ContinuationFrame** — parameteric description of legal next choices for a focused task.
- **Hole** — missing pure value with a persistent ID and required type/shape.
- **EvidenceIR** — deterministic correctness/confidence records with hashes, method, parameters, result and provenance.
- **Equality edge** — compiler-owned positive proof that one whole ImplIR program reaches another through one exact production rewrite.
- **Equality hash** — canonical identity of an equality anchor, hash-consed nodes, proof edges, worklist and status, independent of batching history.
- **Equality node** — one fully verified whole-program ImplIR member, hash-consed by `impl_hash`.
- **Equality space** — bounded persistent positive proof graph rooted at one fully proved unconditional candidate revision.
- **Guarded fallback** — candidate-level compiler guard selecting a conditional primary or immutable proved exact fallback lazily.
- **Impl hash** — history-independent identity of reachable typed ImplIR semantics.
- **ImplIR** — separate typed functional graph describing one implementation of frozen SpecIR.
- **MemoryIR physical boundary** — current Stage 3 layer; ScheduleIR and target lowering remain future work.
- **Obligation** — explicit proposition that is open, proved, refuted or unsupported.
- **Proof debt** — ordered persistent speculative obligations connecting consecutive implementation hashes.
- **Proof frontier** — last consecutive candidate prefix whose exact equivalence has compiler-owned proof; it may lag behind head.
- **Proposal hash** — domain-separated identity of an alpha-normalized replacement proposal before persistent ImplIR ID allocation.
- **Speculative proposal** — typed replacement fragment accepted with explicit opt-in but not treated as correctness evidence.
- **Persistent ID** — compiler-assigned identity such as `v4`, `h1` or `r2`.
- **Region** — closed pure block with typed arguments, explicit captures and a yield.
- **Revision** — immutable workspace snapshot with parent links and content hash.
- **Resource limit** — runtime workload policy excluded from all SpecIR/ImplIR/candidate semantic or exact hashes.
- **ScheduleIR** — future mapping of work to target hardware.
- **SpecIR** — functional graph describing what must be computed.
- **Semantic canonical form** — versioned, alpha-normalized output-reachable representation of frozen SpecIR.
- **Spec hash** — domain-separated SHA-256 identity of semantic canonical form, independent of compiler IDs and construction history.
- **Temporary binding** — `$name` usable only within one transaction.
- **Workspace** — SpecIR revision DAG plus independent CandidateForest, EqualityStore, MemoryPlanStore and compiler-owned allocators; it may be persisted only after complete replay verification.
