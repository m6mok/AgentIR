# Architectural decisions

This log records Stage 1 choices that narrow or defer parts of AgentIR 0.1.

## ADR-001: Rust workspace and transport-independent core

**Decision.** Use stable Rust with four crates: core, evaluator, protocol, and CLI.

**Why.** Rust provides exhaustive tagged enums, strong ownership boundaries and predictable native performance without a runtime or garbage collector. Separating the compiler core prevents stdin/stdout, JSON, MCP, or future daemon details from becoming AgentIR semantics.

**Alternatives.** A single crate would bootstrap faster but blur dependency direction. C++ would fit compiler ecosystems but add memory-safety and build-system cost before Stage 1 needs LLVM integration.

## ADR-002: Persistent IDs and revisions

**Decision.** A workspace owns monotonic typed ID counters (`vN`, `opN`, `hN`, `oN`, `rN`). A transaction stages a cloned allocator and program, so a rejected transaction consumes no IDs and changes no state. Revisions own immutable full snapshots; explicit forks can share a content hash while retaining distinct revision IDs.

`@N` is a one-based index into the deterministic current live-value table and is resolved to a persistent ID before commit. `$N` inside a shape string is codec sugar for the already declared symbol `N`.

**Alternatives.** UUIDs are larger and agent-unfriendly. Content-addressed object IDs would improve construction-order independence but complicate temporary cyclic/partial graphs at this stage.

## ADR-003: Exact revision serialization and content hash

**Decision.** Exact state bytes are compact `serde_json` over deterministic `Program` collections, followed by SHA-256. Float constants are stored as exact lowercase IEEE-754 bit strings. Timestamp and Stage 1.1 semantic-cache metadata live on `Revision`, outside the hashed `Program`.

`content_hash` intentionally remains history-sensitive: action provenance, obligations and compiler-assigned IDs participate. Its behavior is not changed by Stage 1.1 because archive replay and published v1 revision hashes depend on it. Semantic identity is the separate `spec_hash` defined by ADR-012.

## ADR-004: Compact shape solver

**Decision.** Support static dimensions, symbols, and one-symbol affine forms `k*N+c`. Equality returns exactly `proved`, `contradiction`, or `unknown`. Equal expressions are proved; unequal static extents contradict; unrelated symbols remain unknown and create `ShapeCompatible` proof debt.

**Alternatives.** An SMT or general symbolic algebra dependency is explicitly outside Stage 1. `AddConstraint` currently retains facts but does not perform general obligation discharge.

## ADR-005: Division and integer overflow

**Decision.** The reference interpreter rejects both integer and floating division by zero with `DIVISION_BY_ZERO`. Checked i32 arithmetic rejects overflow instead of relying on build-mode behavior. Non-finite f32 results that cannot be represented by the JSON codec are structured evaluation failures.

**Why.** This gives Stage 1 one explicit, build-independent behavior and no hidden undefined behavior. Later numeric contracts may add IEEE division policies.

## ADR-006: Regions and captures

**Decision.** `map`, `zip_map`, and `reduce` own a single pure inline region. Its namespace contains typed block arguments, earlier local SSA bindings, and an explicit allow-list of outer captures. Stage 1 captures must evaluate to scalars. Nested higher-order regions are deferred.

**Why.** This is sufficient for SAXPY's scalar `a` capture while making accidental ambient visibility impossible.

## ADR-007: Typed holes

**Decision.** A hole owns a typed placeholder `ValueId`. Filling records a compatible persistent value; reads resolve the placeholder to that value. Open holes create `HoleFilled` obligations and block freeze/evaluation. Continuation menus include only values whose compatibility is proved, not unknown.

## ADR-008: Revision branching policy

**Decision.** Ordinary transactions require `base_revision == head`. An explicit `allow_branch` flag or `revision.fork` opts into branching. This keeps stale writes distinguishable from deliberate search branches.

## ADR-009: Stage 1 reductions and casts

**Decision.** `reduce` currently reduces the entire dense tensor in fixed row-major order using an explicit scalar identity and `(T,T)->T` combiner. `cast` names a scalar `target_type` and preserves tensor shape. No implicit cast or broadcasting exists.

## ADR-010: Deferred specification surface

The following AgentIR 0.1 areas are intentionally not implemented: `i64`, `f16`, `bf16`, general affine/divisibility proofs, multi-result operations, `scan`, broadcast/reshape and advanced tensor operations, ImplIR, MemoryIR, ScheduleIR, EvidenceIR measurement storage, persistent Workspace DB, GPU codegen, MCP transport, formal solvers, and autotuning.

The Stage 1 brief takes precedence where it defines a smaller profile than the full 0.1 specification.

## ADR-011: Versioned archive and deterministic replay

**Decision.** `agentir-core` exposes an I/O-free `WorkspaceSnapshot` containing schema version, immutable revisions, allocator state and an ordered event log. `agentir-store` wraps it in the versioned `agentir.workspace` envelope, hashes the deterministic version-specific body with SHA-256, writes a same-directory temporary file, calls `sync_all`, and atomically renames it into place. The original writer published v1, Stage 1.1 published v2, and Stage 1.2 publishes v3.

Loading is deliberately expensive and defensive: verify the archive hash, replay every transaction/fork through the normal compiler core, reproduce compiler IDs and revision hashes, recompute every archived program hash and status summary, then publish the restored workspace. Timestamps are restored metadata and are not replay-equivalence inputs. Ephemeral continuation counters are persisted but excluded from graph-event equivalence.

**Alternatives.** Serializing only the latest graph would resume quickly but could not establish provenance. Replaying only a log would lose original timestamp metadata and make random-access reads expensive. A SQLite/RocksDB workspace database is premature until the archive schema and query workload stabilize.

**Evolution.** Archive/snapshot v1 and v2 are frozen legacy codecs. Stage 1.1 introduced explicit v1 → v2 migration and semantic-cache metadata. Stage 1.2 adds the explicit v2 → v3 event-versioning migration described by ADR-015/ADR-016. Every source is checksummed by its exact codec before migration; unknown future versions are rejected.

**Limitations.** Version 2 still has no process locking, directory `fsync`, compression, incremental snapshots or encryption. The local CLI accepts explicit filesystem paths and remains unsuitable as a multi-tenant server boundary.

## ADR-012: Versioned semantic canonical form and spec hash

**Decision.** A complete frozen SpecIR is converted to `SemanticCanonicalProgramV1`, serialized as deterministic compact JSON and hashed with domain separation `agentir.spec.semantic.v1\0`. This is a distinct codec, not serialization of `Program`.

External parameters and outputs are sorted by name and retain their names/types. Only the output-reachable operation DAG is emitted. Traversal preserves output, operand and region execution order; it assigns compiler-independent `p*` and `n*` references. Symbolic dimensions become `d*`, region arguments `%arg*`, and local results `%local*`. Actual outer uses are canonical references, while unused capture allow-list entries, unreachable internal graph, provenance, obligations and allocator state are absent. `NumericContract` remains semantic.

Potential persistent references inside generic semantic attributes are rejected with `CANONICALIZATION_FAILED` until an opcode-specific canonical resolver exists. This conservative failure is preferable to hashing a compiler ID.

**Limitation.** This establishes ordered typed graph isomorphism/history independence, not algebraic equivalence. Commutative sorting, reassociation, CSE equivalence and `mul+add`/`fma` equivalence are explicitly out of scope. Shared and duplicated graphs remain distinct.

## ADR-013: Explicit archive migration registry

**Decision.** `agentir-store` keeps separate v1/v2/v3 envelope types and an ordered registry with `workspace_archive_v1_to_v2` and `workspace_archive_v2_to_v3`; v3 → v3 is an explicit reported no-op. The read pipeline is bounded read → version sniff → exact codec → source checksum → pure migration → current schema replay → cached semantic verification.

`workspace.migrate_archive` fully validates the source before checking/writing the destination, performs migration in memory, and uses the existing same-directory atomic writer. Existing or in-place destinations require `overwrite: true`. A failure never publishes a workspace and does not leave a partial destination.

**Alternatives.** Deserializing v1 into v2 with serde defaults was rejected because it would make future compatibility implicit and could validate an archive using the wrong hash rules. Mutating source files in place was rejected because it weakens recovery and auditability.

## ADR-014: Compact deterministic constraint facts

**Decision.** Stage 1.2 derives an immutable `ConstraintFacts` view from declared dimensions and accepted shape constraints. The engine uses deterministic lexical representatives for symbol equivalence, propagates symbol-to-static bindings, proves identical normalized one-symbol affine expressions, and reports `proved`, `contradiction`, or `unknown`. Equalities outside those compact rules remain `unknown` even when a stronger solver could decide them. Duplicate facts are idempotent.

**Why.** Sound incremental discharge is required before implementation search can rely on proof debt. A small `BTreeMap`/`BTreeSet` model keeps proof results and diagnostics reproducible and auditable.

**Alternatives.** SMT, Presburger arithmetic, divisibility reasoning, nonlinear algebra and general inequality solving are deferred. Stage 1.2 prefers an incomplete sound answer to an opaque or accidentally unsound proof.

## ADR-015: Event-level compiler semantics versions

**Decision.** Compiler semantics and archive format are independent version axes. `LEGACY_CORE_SEMANTICS_VERSION = 1` reproduces Stage 1.1 transaction behavior byte-for-byte; `CORE_SEMANTICS_VERSION = 2` enables constraint validation and structured shape-obligation discharge. Every current event stores its semantics version. Migrated v1/v2 events receive version 1, while newly accepted events receive version 2, including when they extend a migrated workspace.

**Why.** `Program` obligations and provenance participate in `content_hash`. Replaying a historical transaction with newer verifier behavior can therefore change IDs, propositions and hashes even when its mathematical graph is unchanged. Event-level versioning preserves the exact historical contract without applying new semantics retroactively.

## ADR-016: Archive and snapshot version 3

**Decision.** Archive v1 and v2 remain immutable source codecs. The current writer emits archive v3 with snapshot schema v3 and `VersionedWorkspaceEvent`. Loading follows v1 → v2 → v3 or v2 → v3 migrations after verifying the exact source archive hash. Saving a restored workspace always emits v3; mixed semantics histories remain explicit.

**Why.** An event semantics discriminator is replay state, not optional metadata. A new schema and explicit migration make its introduction reviewable and prevent serde defaults from silently changing compatibility behavior.

## ADR-017: Central resource limits and hard safety caps

**Decision.** `ResourceLimits`, `ResourceKind`, `ResourceUsage` and `BudgetCheck` define deterministic limits shared across core, evaluator, store, protocol and CLI. Interactive limits are configurable and excluded from program/archive identities. Archive migration and replay use larger non-configurable hard safety caps so lowering an interactive limit cannot make a previously accepted legacy archive unreplayable. Limits are checked before parsing, graph cloning, persistent-ID allocation, replay and tensor output allocation wherever the boundary exposes the projected size.

**Why.** Bounded individual components are insufficient if requests, replay and evaluation each use unrelated policies. Structured `RESOURCE_LIMIT_EXCEEDED` failures keep rejection atomic and repairable.

## ADR-018: Statistical dependency-light benchmark baseline

**Decision.** Baseline schema v2 runs warm-ups plus repeated samples and reports min, median, p95 and max with workload size, units and build/host metadata. Canonical byte sizes are reported separately from timings. Timing changes never fail CI.

**Why.** Single-shot nanosecond values are too noisy for regressions or architectural comparisons. Median describes the typical local run and p95 exposes tail instability without pretending that the reference interpreter is a GPU benchmark.

**Alternatives.** Criterion and a heavyweight fuzz framework remain deferred. Fixed-seed bounded property/mutation corpora and a small standard-library timing harness provide reproducible Stage 1.2 coverage with minimal dependencies.

## ADR-019: ImplIR is a separate typed graph

**Decision.** Stage 2A introduces `ImplProgram`, `ImplOperation`, `ImplValue`, `ImplRegion` and `ImplOutput` as distinct types. A verifier/evaluator adapter may translate them into the shared pure operation semantics internally, but ImplIR is never a `Program` alias or a mode bit on SpecIR. Identity lowering copies the complete reachable ordered computation and external interface while issuing independent `iop*`/`iv*` IDs and source links.

**Why.** SpecIR remains the immutable statement of what to compute. Implementation provenance, revision history and later algorithm choices must evolve without mutating or semantically overloading that contract.

## ADR-020: CandidateForest and candidate revision DAG

**Decision.** A workspace owns an independent `CandidateForest`. Each `c*` branch anchors one frozen SpecIR revision and immutable `spec_hash`, owns a `cr*` revision DAG and uses a candidate-only monotonic allocator. Candidate transactions name an explicit base candidate revision and stage the entire forest, so rejection consumes no IDs or evidence and changes no head. Forking creates a new candidate identity; a fork of a sealed revision is editable `draft` state while retaining the verified proof chain.

## ADR-021: Exact compiler-owned known rewrites only

**Decision.** Stage 2A accepts `prune_unreachable_impl_nodes`, fully type-identical `eliminate_noop_cast`, and defined scalar constant folds for the implemented primitive subset. Matching, side-condition discharge, transformation and certificate construction are compiler-owned. Overflow, zero division, non-finite f32 folds, stale hashes, unknown rules and non-matches reject atomically.

**Deferred.** Agent-supplied `ReplaceSubgraph`, speculative rewrites, reassociation, commutation, `x+0`, FMA contraction/expansion, reduction reordering, e-graphs and saturation belong to Stage 2B or later.

## ADR-022: Compositional exact equivalence certificates

**Decision.** `EquivalentToSpec` is proved by an identity-lowering certificate followed by zero or more trusted known-rewrite certificates. Every edge records before/after `impl_hash`, targets, discharged conditions, semantics version and EvidenceIR reference. Verification recomputes identity lowering, checks every link and requires the terminal hash to equal current ImplIR. Testing never closes this obligation.

## ADR-023: Correctness and confidence EvidenceIR are distinct

**Decision.** Identity, known-rewrite and compositional certificates are correctness evidence. Fixed-seed differential/property tests are confidence evidence. Evidence is deterministic and records hashes, candidate/revision, method, normalized parameters, result and compiler semantics; no wall-clock timestamp enters candidate identity. Stage 2A stores no performance evidence.

## ADR-024: Five non-substitutable hash contracts

**Decision.** `Revision.content_hash` remains the exact history-sensitive SpecIR `Program` hash. `spec_hash` remains the history-independent frozen-SpecIR semantic hash. `impl_hash` uses domain `agentir.impl.semantic.v1\0`, ignores implementation IDs/source links/evidence/unreachable nodes and preserves interface, ordered operands, regions, types, constraints and `NumericContract`. `candidate_hash` uses domain `agentir.candidate.exact.v1\0` and covers exact candidate IDs, SpecIR anchor, full ImplIR IDs/state, proof chain and evidence references while excluding time/resource policy. `archive_hash` protects one exact versioned envelope. None substitutes for another.

## ADR-025: Candidate semantics is an independent version axis

**Decision.** Candidate events use `CANDIDATE_SEMANTICS_VERSION = 1`, independent of SpecIR core semantics v1/v2, ImplIR semantics v1, canonical codecs and archive format. Replay dispatches candidate history only after all SpecIR history and hashes verify.

## ADR-026: Archive and snapshot version 4

**Decision.** Archive/snapshot v1, v2 and v3 remain immutable legacy codecs. The current writer emits v4 containing the unchanged SpecIR revision DAG/event log plus `CandidateForest`, candidate allocator, EvidenceIR and versioned candidate events. Explicit v3 → v4 migration adds empty candidate state and creates no candidates. Publication follows complete SpecIR replay, candidate replay, all hash/certificate checks and forest consistency verification.

## ADR-027: Stage 2A stops before memory, schedule and approximate refinement

**Decision.** Stage 2A has no buffers, layouts, address spaces, raw pointers, target manifests, schedules, threads, tiles, GPU/backend integration, search/ranking, hardware benchmarks or error tolerances. Approximate refinement returns `UNSUPPORTED_REFINEMENT`. These boundaries keep the first candidate/equivalence layer auditable before physical lowering and speculative search are introduced.
