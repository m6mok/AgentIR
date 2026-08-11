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

## ADR-028: Speculative acceptance requires explicit opt-in

**Decision.** Conditional, unknown and unsupported typed proposals are accepted only with `allow_speculative: true`. Illegal proposals are always rejected atomically. Well typed means structurally executable, not equivalent, and a proposal is provenance rather than correctness evidence.

## ADR-029: Proposal fragments have an exact operation boundary

**Decision.** Stage 2B replaces one top-level single-result operation. The fragment declares exactly the target's ordered operands, uses only boundary or earlier `$` bindings, contains pure existing ImplIR operations and yields one exactly typed result. The core assigns all persistent ImplIR IDs. Parameters, outputs, constraints and `NumericContract` cannot change.

## ADR-030: Proposal hash is a separate semantic contract

**Decision.** Alpha-normalized proposals use domain `agentir.proposal.semantic.v1\0` and cover the base `impl_hash`, target/boundary, replacement, output type, numeric contract and codec version. Candidate identities, allocated IDs, evidence, timestamps and limits are excluded. `proposal_hash` never substitutes for `impl_hash` or `candidate_hash`.

## ADR-031: Proof frontier and ordered proof debt

**Decision.** Every speculative acceptance appends one obligation connecting consecutive implementation hashes. The candidate head may advance beyond the last consecutively proved frontier. Validators process debt in order; open, unsupported or refuted items prevent later items from advancing the frontier. Testing never discharges debt.

## ADR-032: Translation proofs are compiler owned

**Decision.** The trusted validator recognizes only equal implementation hashes, an exact result reproduced by the production known-rewrite transform, or ADR-034's guarded profile. Agent rule names and certificates are ignored. Unsupported checks are persisted deterministically without correctness evidence and without being treated as semantic failure.

## ADR-033: Deterministic counterexamples refute obligations

**Decision.** Positive differential/property validation is confidence evidence. Its first deterministic mismatch records a bounded normalized counterexample, marks the first unresolved affected obligation `refuted`, rejects the candidate and leaves the proof frontier unchanged. A refuted candidate cannot be sealed.

## ADR-034: One restricted lazy guarded fallback

**Decision.** The sole Stage 2B guarded rule is scalar `i32 div(x,x) -> 1` under compiler-owned `I32NonZero(x)`. False evaluates an immutable fully proved exact fallback revision lazily; true evaluates the primary. Guard dependencies, fallback recursion and cycles are bounded. No general guard DSL is introduced.

## ADR-035: Candidate hash v1 and v2 coexist

**Decision.** Candidate canonical/hash v1 and domain `agentir.candidate.exact.v1\0` are immutable. New speculative or guarded revisions use v2 and `agentir.candidate.exact.v2\0`, adding normalized proposal records, frontier, ordered debt/statuses, translation results, guard/fallback and lifecycle state. Each revision names its hash version; migrated ancestors retain v1 bytes and hashes.

## ADR-036: Candidate semantics v2 is independent

**Decision.** Proposal acceptance, translation results and Stage 2B evidence use `CANDIDATE_SEMANTICS_VERSION = 2`; legacy candidate events remain version 1. Candidate semantics, core semantics, ImplIR semantics, canonical versions and archive versions are separate compatibility axes, and one forest may replay mixed v1/v2 candidate history.

## ADR-037: Archive and snapshot version 5

**Decision.** Archive/snapshot v1-v4 are exact legacy inputs. V5 adds proposals, proof debt, guards and candidate semantics v2. Migration verifies the v4 body with immutable v4 types, preserves SpecIR state and all candidate v1 IDs/hashes/evidence/events, adds empty Stage 2B stores and never recalculates legacy candidate hashes. New saves write only v5.

## ADR-038: Agents cannot supply correctness certificates

**Decision.** Protocol proposal types expose no correctness EvidenceIR, rewrite certificate or guard field. Only compiler-owned identity lowering, production transforms and the guarded validator allocate correctness evidence. This prevents plausible agent assertions from crossing the trust boundary.

## ADR-039: Stage 2B excludes approximation and search machinery

**Decision.** Stage 2B adds bounded speculative history, not approximate refinement, tolerances, SMT, e-graphs, saturation, ranking, beam/population search, learned cost models or performance/hardware evidence. MemoryIR, ScheduleIR and GPU/LLVM/MLIR lowering remain later independent stages.

## ADR-040: Equality nodes are whole verified ImplIR programs

**Decision.** Stage 2C stores one complete verified `ImplProgram` per equality node. It does not introduce expression e-classes, congruence closure or a second IR. Equality spaces are anchored to one fully proved unconditional candidate revision and represent positive reachability under trusted exact rewrites.

**Why.** Whole-program nodes reuse the existing verifier, semantic hash and proof contracts while keeping the first saturation layer small enough to replay exhaustively.

## ADR-041: Equality is positive-only trusted saturation

**Decision.** An edge exists only when the compiler applies a production exact rewrite and discharges its side conditions. Absence of a node/path means unresolved, not disequal or refuted. Saturation is fuel/resource bounded and `fixed_point` refers only to the reachable space under the current registry.

## ADR-042: Candidate and equality use one production rewrite engine

**Decision.** Known candidate rewrites, translation recognition, continuations and equality expansion share stable target locators, match enumeration and transforms. No equality-only rewrite implementation or agent-supplied rule/certificate path exists.

**Why.** A single engine prevents a proof edge from describing behavior different from the transaction used to materialize it.

## ADR-043: Equality nodes hash-cons by impl_hash

**Decision.** `impl_hash` is the unique semantic node key. Independent rewrite orders that reach the same reachable typed implementation merge into one node, while distinct trusted proof descriptors remain as deduplicated edges. Self edges are suppressed.

**Why.** `impl_hash` already excludes IDs, provenance and unreachable nodes while preserving the exact interface, types, regions, ordered operands, constraints and numeric contract needed for this identity.

## ADR-044: Candidate canonical/hash and semantics v3

**Decision.** Candidate v1/v2 codecs and hashes remain immutable. Revisions containing equality membership or materialization records use domain `agentir.candidate.exact.v3\0` and candidate semantics v3. V3 covers equality space/revision/hash, endpoint hashes, canonical path digest, ordered proof edge IDs and linked evidence/materialization provenance.

## ADR-045: Equality event dependencies require archive v6

**Decision.** Equality events use independent semantics v1 and record the candidate-event cursor on which they depend. Replay interleaves candidate/equality histories at those cursors. Archive/snapshot v6 adds EqualityStore and native equality state; explicit v5 → v6 migration verifies immutable v5 and adds an empty store without changing legacy bytes or hashes.

## ADR-046: Equality members materialize only through explicit candidate transactions

**Decision.** The compiler never extracts or ranks an equality member. `equality.materialize` names one node, forks the immutable anchor and replays its canonical trusted path through ordinary `CandidateAction::ApplyKnownRewrite`, then verifies the terminal `impl_hash`. Equality-local IDs are not copied into CandidateForest.

**Why.** Reusing the atomic CandidateForest engine preserves allocation, verification, evidence and replay invariants and makes selection policy an explicit client concern.

## ADR-047: Stage 2 completes at the exact-only equality boundary

**Decision.** Stage 2C contains deterministic bounded equality exploration, proof explanation, debt discharge and explicit materialization. It contains no approximate relation, e-graph, extractor, ranking, beam/population search, learned cost model, performance evidence, MemoryIR, ScheduleIR or target lowering. Stage 3 begins with MemoryIR.

## ADR-048: MemoryIR is a separate typed graph

**Decision.** Stage 3 stores physical decisions in `MemoryProgram`, never as ImplIR attributes. Each plan anchors one immutable frozen SpecIR/candidate/ImplIR triple and evolves through an independent `mp*`/`mr*` revision DAG. SpecIR and ImplIR remain functional and immutable.

## ADR-049: High-level typed regions, not raw pointers

**Decision.** Buffers are abstract typed regions with element type, shape, exact logical strides, layout, address space, access, ownership, alignment, lifetime and alias domain. Access is a region plus typed logical index. Raw pointers, byte-address arithmetic, casts and backend capacity claims are excluded.

## ADR-050: Conservative fresh bufferization is the exact baseline

**Decision.** Tensor inputs borrow read-only external regions, constants use immutable constant regions, and tensor results receive distinct writable plan-owned regions in reachable operation order. Scalar SSA is retained. Immutable fresh result templates remain available as repair and guarded fallback.

## ADR-051: Lifetimes precede schedules only as logical facts

**Decision.** First use, ordered uses, last use, escape and release eligibility use canonical high-level operation order. They prove single-threaded MemoryIR storage legality only and make no claim about future ScheduleIR order, races, target concurrency, or performance.

## ADR-052: Alias provenance is compiler owned

**Decision.** The core derives `must_alias`, `no_alias`, `may_alias` and `partial_overlap` with explicit provenance. Agent-supplied proofs and guards do not exist in the protocol. `unverified_claim` can be retained for audit but never closes a reuse obligation.

## ADR-053: Static reuse requires a complete structural proof

**Decision.** In-place reuse requires identical tensor type/shape, compatible layout/strides/alignment, writable plan ownership, last use at overwrite, no old-value escape and no overlapping live reader. Rejection is atomic and recommends the exact fresh baseline; testing cannot authorize reuse.

## ADR-054: One compiler-owned NoOverlap memory guard

**Decision.** Guarded memory reuse supports only `NoOverlap` over trusted typed runtime region metadata. The true reuse path is structurally verified and the false path lazily allocates an immutable proved fresh template. Both preserve the anchored `impl_hash`; no guard DSL or pointer comparison is introduced.

## ADR-055: Memory hash is an independent exact-state contract

**Decision.** `memory_hash` v1 uses domain `agentir.memory.exact.v1\0` and covers anchors, typed physical graph, analysis facts, decisions, proof references and lifecycle. It excludes timestamps, resource policy and platform state. Legacy content/spec/impl/proposal/candidate/equality hashes are unchanged and non-substitutable.

## ADR-056: Memory events require archive and snapshot v7

**Decision.** Memory events use semantics v1 and explicit candidate/equality dependency cursors. Replay restores SpecIR, interleaves CandidateForest/EqualityStore, then rebuilds memory-local IDs and verifies every plan, certificate and hash. Immutable v6 decoding and explicit v6 → v7 migration add an empty MemoryPlanStore without changing legacy state or hashes; new saves use v7.

## ADR-057: Stage 3 stops before scheduling and targets

**Decision.** Stage 3 completes exact logical-to-physical bufferization, alias/lifetime legality, reuse and reference tracing. ScheduleIR, TargetManifest, tiling, binding, vectorization, device execution, ranking, search, performance evidence and backend lowering remain Stage 4 or later.

## ADR-058: ScheduleIR is a separate typed graph

**Decision.** Scheduling decisions live in an independent typed graph anchored to one proved MemoryIR revision, never in SpecIR, ImplIR, or MemoryIR attributes.

## ADR-059: TargetManifest is immutable

**Decision.** A target revision is sealed at creation. Schedule histories anchor its exact revision and hash; capability mutation requires a new manifest.

## ADR-060: Target capabilities are compiler owned

**Decision.** Clients select a stable built-in profile and cannot provide capabilities, capacities, guards, or target certificates.

## ADR-061: Every plan begins with a conservative serial root

**Decision.** The root follows canonical MemoryIR operation order, binds logical axes serially, and preserves reduction order. It is the exact repair for unsupported transforms.

## ADR-062: Iteration coverage is exact

**Decision.** Compiler-derived root domains and active transform leaves must execute every logical coordinate once, without omission or duplication.

## ADR-063: Remainder handling is compiler owned

**Decision.** Non-divisible and symbolic split/tile operations create an exact compiler-owned remainder domain. Clients cannot supply tail predicates.

## ADR-064: Fusion is deliberately restricted

**Decision.** Stage 4 fuses only one dependent single-user pointwise producer/consumer pair with identical domains and no conflicting memory facts.

## ADR-065: Binding uses a typed hierarchy

**Decision.** Serial, grid, block/workgroup, subgroup, and vector-lane bindings are distinct typed choices checked against TargetManifest; vector lanes arise only from verified vectorization.

## ADR-066: Vectorization requires structural legality

**Decision.** Supported width and scalar type are insufficient alone: every affected MemoryIR buffer must also prove compatible innermost stride and alignment. Testing cannot authorize vector access.

## ADR-067: Scheduling preserves MemoryIR facts

**Decision.** Schedule verification rebuilds dependencies and rejects any order or transform that invalidates alias, lifetime, access, reuse, or guarded-fallback facts. Schedule edits never alter `memory_hash`.

## ADR-068: Resource simulation is deterministic and analytical

**Decision.** The simulator recomputes capacity usage from ScheduleIR and TargetManifest with bounded checked/saturating arithmetic. It is a legality check, not a cost model or equivalence proof.

## ADR-069: Target hash is independent

**Decision.** `target_hash` v1 uses domain `agentir.target.manifest.v1\0`, covers the immutable capability contract, and excludes limits, timestamps, discovery, and performance data.

## ADR-070: Schedule hash is independent

**Decision.** `schedule_hash` v1 uses domain `agentir.schedule.exact.v1\0`, covers exact schedule/proof/resource state and immutable anchors, and excludes limits, timestamps, runtime inputs, traces, and measurements.

## ADR-071: Target and schedule events require archive v8

**Decision.** Archive/snapshot v8 adds immutable target and schedule stores. Schedule events record dependency cursors through candidate, equality, memory, and target histories; replay verifies all anchors, certificates, estimates, hashes, allocators, and order before publication.

## ADR-072: Stage 4 ends before backend lowering

**Decision.** Stage 4 completes exact high-level scheduling, target contracts, resource simulation, and reference execution. Backend IR, machine code, device execution, target discovery, autotuning, ranking, search, and performance evidence begin in Stage 5 or later.

## ADR-073: BackendIR is separate and schedule anchored

**Decision.** Stage 5 stores typed kernels, bindings, expressions, statements and dispatches in an immutable BackendIR plan anchored to one exact ScheduleIR revision. WGSL source is an output artifact, never the canonical program.

## ADR-074: The first backend is a deliberately small WebGPU profile

**Decision.** `webgpu_wgsl_v1` supports exact one-dimensional f32 elementwise kernels, deterministic storage/uniform ABI, serial or grid execution, widths 1/2/4, restricted fusion and compiler-owned bounds/NoOverlap handling. Unsupported reductions, layouts, address spaces and GPU features reject structurally.

## ADR-075: Backend and artifact hashes are independent contracts

**Decision.** `backend_hash` covers typed BackendIR and its proof state; `artifact_hash` covers the complete manifest and exact WGSL bytes. Neither includes runtime limits, device fingerprints or measurements, and neither substitutes for any earlier hash.

## ADR-076: WGSL validation and devices have no correctness authority

**Decision.** Naga parsing/validation establishes artifact well-formedness. Device execution and benchmarks are confidence evidence only. `BackendEquivalentToSchedule` and `ArtifactEquivalentToBackend` are compiler-owned structural certificates.

## ADR-077: Executable packages retain the complete binding ABI

**Decision.** Every artifact entry point retains ordered storage bindings, parameter block, logical extent and output mappings. The runtime consumes this manifest directly and never reverse-engineers ABI or semantics from WGSL text.

## ADR-078: Backend events require archive v9

**Decision.** Snapshot/archive v9 adds BackendStore, ArtifactStore and MeasurementStore. Loading verifies exact hashes, certificates, WGSL/manifest consistency and dependency order before publication. Immutable v1–v8 inputs cross explicit migrations; new saves use v9.

## ADR-079: Hardware measurements remain provenance-rich observations

**Decision.** A measurement anchors artifact, target, compiler build and device fingerprint plus bounded configuration and timing statistics. It cannot rank artifacts, mutate ScheduleIR or advance equivalence.

## ADR-080: Compiler, emitter and runtime remain separate crates

**Decision.** Transport-independent BackendIR, hashes, stores and replay stay in `agentir-core`; `agentir-backend-wgsl` owns deterministic lowering/emission/offline Naga validation; `agentir-runtime-wgpu` alone owns adapters, devices, buffers, pipelines and readback. Core has no wgpu/Naga/device/OS dependency.

## ADR-081: Lowering and all correctness certificates are compiler owned

**Decision.** Protocol clients select immutable IDs/hashes and runtime inputs only. Kernel boundaries, BackendIR values/statements, ABI, dispatch formulas, bounds/guard predicates and both Stage 5 certificates are constructed and verified by trusted compiler code; arbitrary WGSL and external shader modules are rejected by request decoding.

## ADR-082: The serial root is the exact backend baseline

**Decision.** Every supported one-dimensional elementwise serial ScheduleIR root lowers to one deterministic dispatch containing a bounded fixed-order loop. Unsupported higher-performance schedules remain legal ScheduleIR and return a structured lowering rejection; Stage 5 does not mutate or repair them.

## ADR-083: Bounds, tiles and remainders are retained structurally

**Decision.** Grid/workgroup lowering derives invocation indices, workgroup counts and exact compiler-owned bounds/remainder predicates from verified ScheduleIR. Clients cannot provide these formulas, and checked/saturating arithmetic plus resource bounds precede publication.

## ADR-084: Fusion is explicit and multi-dispatch order is semantic

**Decision.** Only ScheduleIR fusion groups that re-pass structural coverage/dependency checks form one kernel. All other operations retain deterministic ordered dispatches; no emitter or runtime heuristic fuses, reorders, ranks or drops them.

## ADR-085: Vector and unroll choices are exact BackendIR metadata

**Decision.** Widths 1/2/4 and bounded unroll factors are copied from verified ScheduleIR into each typed kernel and its certificate/query surface. Width 8 and unsupported ABI/layout cases reject. Scalar WGSL accesses remain the conservative exact execution form for the v1 emitter; vector metadata does not authorize reassociation or approximate math.

## ADR-086: Memory reuse and guards are not re-proved by the backend

**Decision.** Static read-write bindings require existing MemoryIR reuse certificates. Guarded packages retain only compiler-owned `NoOverlap`, explicit true/fallback dispatch selections and the exact fresh fallback provenance; runtime chooses one branch and execution traces record the outcome without changing artifact identity.

## ADR-087: WGSL bytes are deterministic portable output

**Decision.** Stable identifiers, declaration/binding/module order, LF whitespace and numeric encoding define WGSL bytes. Timestamps, paths, diagnostics, device data, driver binaries and pipeline caches are excluded. Re-emitting identical BackendIR with one compiler build is byte-identical.

## ADR-088: Compiler build identity is separate from artifacts and backends

**Decision.** `compiler_build_hash` uses its own versioned domain and records emitter/validator compatibility. It participates in `artifact_hash` and measurement provenance but never substitutes for `backend_hash`, earlier correctness hashes, or a device fingerprint.

## ADR-089: Device discovery is mutable runtime state only

**Decision.** Adapter limits are checked against immutable `webgpu_wgsl_v1`; discovery cannot mutate TargetManifest or `target_hash`. Device fingerprints and execution results remain outside backend/artifact correctness and archive replay never opens hardware.

## ADR-090: Stage 5 completes without comparative search

**Decision.** Stage 5 explicitly lowers/emits/executes the client-selected ScheduleIR revision and records bounded measurements. It has no autotuning, cost model, best-plan/artifact selection, beam/population search, learned policy, profile-guided specialization or performance-derived correctness claim; those policies begin no earlier than Stage 6.

## ADR-091: Evaluation is a separate non-correctness layer

**Decision.** Stage 6A records interaction efficiency above the production protocol. Evaluation success, replay, metrics, tokens, testing, and performance observations never create or strengthen compiler evidence.

## ADR-092: Policy evaluation has a separate crate

**Decision.** `agentir-policy-eval` owns corpus, episodes, transcripts, replay, aggregates, comparisons, and evaluation archives. Core remains unaware of experiment orchestration and provider metadata.

## ADR-093: Task corpora are immutable and ordered

**Decision.** Exact ordered task definitions, including fixed task budgets and success criteria, are versioned and covered by `corpus_hash`. Reordering changes identity.

## ADR-094: Free, menu, and hybrid expose distinct surfaces

**Decision.** Free exposes the production schema without choices; menu accepts only generated choice IDs; hybrid adds one bounded typed escape. Every resolved action uses the same production verifier and atomic transaction path.

## ADR-095: Compiler outcomes and success are harness owned

**Decision.** Clients cannot submit success, rejection classes, outcomes, hashes, semantic scores, metrics, guards, or certificates. The harness derives them from structured production responses and task criteria.

## ADR-096: Transcripts replay without an agent or device

**Decision.** Replay resolves recorded decisions, rebuilds fresh production sessions, and compares exact structured outcomes. It performs no provider, network, model, GPU, adapter, or benchmark call.

## ADR-097: Token sources retain explicit trust

**Decision.** Deterministic bytes/tokens, provider reports, and agent self-reports are separate. Missing values remain unknown and externally reported values are provenance, not trusted correctness data.

## ADR-098: Evaluation hashes are independent

**Decision.** Corpus, policy, observation, episode, aggregate/evaluation, and evaluation archive v1 use distinct `agentir.evaluation.*.v1` domains. None substitutes for or enters a compiler hash.

## ADR-099: Evaluation archives are separate from workspace v9

**Decision.** `agentir.evaluation.archive` v1 stores only reproducibility manifests, corpus/policy descriptors, transcripts/outcomes, raw aggregates, and optional hardware anchors. Workspace archive v1–v9 codecs remain unchanged.

## ADR-100: Comparisons enforce exact fairness anchors

**Decision.** Corpus, compiler build, task definitions/budgets, seed set, initial state, runtime inputs, criteria, and—when relevant—device fingerprint must match. Failed episodes remain visible and no implicit weighted score is produced.

## ADR-101: Scripted policies are deterministic controls

**Decision.** Five named scripted baselines cover CI and harness replay. They make no learned-policy claim and perform no hidden ranking.

## ADR-102: Hardware observations remain separate

**Decision.** Optional hardware observations require proved/offline-valid artifacts and retain artifact, measurement, and device hashes. They are comparable only on the same device fingerprint and never advance proof.

## ADR-103: Stage 6A stops before tuning and ranking

**Decision.** Stage 6A contains no autotuning, learned policy/ranking, prompt optimization, cost model, beam/population search, automatic extraction, schedule mutation, or best-artifact selection. These begin no earlier than Stage 6B.

## ADR-104: Multi-choice continuations are exact visible production actions

**Decision.** Stage 6B expands bounded compiler continuation descriptors into an ordered `EvaluationChoiceSet`. Every choice carries a production request; a parametric domain is never represented as a manually ranked task script.

## ADR-105: Choice IDs are harness assigned from compiler state

**Decision.** Stable choice IDs cover compiler layer/category, typed action, visible preconditions and compiler order. Policies cannot submit IDs, legality, bases, hashes, proof effects or outcomes.

## ADR-106: Ranking features use a visible versioned schema

**Decision.** Feature schema v1 declares exact ordered definitions, types, visibility and normalization. Hidden state, future outcomes, task success, unavailable measurements and reference solutions are forbidden.

## ADR-107: Ranking is policy owned and non-correctness

**Decision.** Scores and preferences influence only explicit selection. They never legalize an action, close an obligation, create EvidenceIR, or turn a measurement into proof.

## ADR-108: Scores are signed fixed-point integers

**Decision.** V1 scores are checked `i64` units at scale 1,000,000 with a bounded magnitude. Platform floats, NaN and infinity are absent from sorting and identity.

## ADR-109: Tie breaking is deterministic

**Decision.** Higher score ranks first; equal scores resolve by compiler order and then stable choice ID. The exact rule is policy-hashed and replay-verified.

## ADR-110: Selection precedes compiler mutation

**Decision.** Ranking is read-only. Only an explicit member selection creates a production request, which traverses `agentir-protocol::Engine`; stale frames and non-members reject before mutation.

## ADR-111: Hybrid ranked escape remains separate and bounded

**Decision.** Escape is outside the compiler choice set, explicitly marked, limited by the typed Stage 6A surface, and receives no precomputed legality or proof authority.

## ADR-112: Ranking hashes are independent

**Decision.** Choice set, feature schema, ranking policy, ranking trace and selection use distinct v1 domains. Ranked episodes use episode v2; none substitutes for a compiler hash.

## ADR-113: Evaluation archive v2 stores ranking records

**Decision.** New evaluation saves use archive v2 with visible schemas, policy descriptors, exact choice sets, traces, selections and explicit per-episode ranking status. Workspace archive v1–v9 is unchanged.

## ADR-114: Evaluation archive v1 migrates without invented ranking

**Decision.** The pure v1→v2 edge verifies the exact v1 envelope first, adds empty ranking stores, and marks every legacy episode `unranked`.

## ADR-115: Ranked comparison requires identical visible experiments

**Decision.** In addition to Stage 6A anchors, comparable runs require identical ordered choice-set hashes, feature-schema identity and permitted escape surface. Incompatibility is explicit.

## ADR-116: Scripted rankers are controls

**Decision.** Seven deterministic baselines exercise fixed-point scores, explicit selection, ties, seed handling and hybrid escape in CI. They make no learned-policy or cost-model claim.

## ADR-117: Stage 6B stops before learned ranking and tuning

**Decision.** Stage 6B contains no training, neural ranking, cost-model fitting, prompt optimization, Bayesian/beam/population search, autotuning, hardware-driven mutation, or automatic fastest-artifact selection.

## ADR-118: Stage 6C learning remains evaluation-only

**Decision.** Datasets, labels, training, models, inference, scores, work counters and held-out metrics live only in `agentir-policy-eval`. They have no legality, proof, success or artifact-selection authority and enter no compiler graph or hash.

## ADR-119: Learned inputs are exact visible frames

**Decision.** Inference receives only the versioned Stage 6B visible schema, exact ordered choices/features/compiler order, complete/bounded status and permitted surface. Labels, future outcomes, reference solutions, policy scores, provider data and split membership are rejected.

## ADR-120: Dataset splits use stable semantic groups

**Decision.** Train/validation/test/excluded membership is a fixed-seed function of semantic group identity, never random per row. One semantic state cannot cross split boundaries.

## ADR-121: The first learned model is an integer pairwise linear ranker

**Decision.** V1 uses a bounded pairwise integer perceptron and deterministic visible-feature codec. Checked integer arithmetic avoids floating nondeterminism and removes native ML, Python, network and GPU dependencies.

## ADR-122: Training is restartable and compiler-independent

**Decision.** Dataset, split, configuration, checkpoint, run and model have independent hashes. Exact epoch/update/work/byte limits and fixed ordering define completion; wall time and environment variables do not.

## ADR-123: Learned inference precedes one production dispatch

**Decision.** A learned descriptor has an explicit policy kind and model binding. Scores traverse existing Stage 6B validation/ties; only the later selected member traverses the production verifier once. Failed inference publishes no trace/selection and mutates no compiler state.

## ADR-124: Learned identities are independent

**Decision.** Dataset, example, semantic group, split, training configuration, checkpoint, training run, model, input and inference use distinct v1 domains. Work counters are non-semantic and excluded from training-run/inference identity.

## ADR-125: Evaluation archive v3 retains learning provenance

**Decision.** New evaluation saves use v3. Immutable v1/v2 inputs migrate only v1→v2→v3; migration invents no learning data and marks legacy episodes unlearned. Replay recomputes inference but never trains or contacts external/device services.

## ADR-126: Resumable enumeration uses opaque anchored cursors

**Decision.** V1 pagination retains exact anchors, kind, limits, count, complete/bounded and exhausted status, version, digest and deterministic work. Choice IDs are assigned before paging; corrupt, future or stale cursors reject before publication.

## ADR-127: Repairs are typed but non-authoritative

**Decision.** Twelve stable repair categories anchor an exact diagnostic/base and carry a bounded ordinary production request. Anchor changes invalidate them, acceptance is not promised, and agent-supplied proofs/guards/certificates are forbidden.

## ADR-128: Stage 7 waits for an explicit freeze gate

**Decision.** The dependency matrix, contract registry, diagnostic/limit/cursor/learning/archive audits, tests and two-run study must be documented before Stage 7. Stage 7 cannot silently change registered Stage 1–6 contracts.

## ADR-129: Stage 7A is narrow offline bounded search

**Decision.** Stage 7A owns only deterministic offline orchestration over existing production-generated menu choice sets. It is not full autotuning, stochastic/population search, hardware selection, approximate equivalence or live workspace publication.

## ADR-130: Search remains evaluation-only

**Decision.** Objectives, plans, frontiers, branch isolation, checkpoints, results and replay live only in `agentir-policy-eval`. Core knows none of these types and search creates no legality, proof, guard, certificate or success authority.

## ADR-131: The first algorithm is deterministic beam v1

**Decision.** `deterministic_beam_v1` is level-synchronous with versioned width/depth/child/cadence semantics. It expands exact total-order frontiers and has no time-, thread-, address-, map-iteration- or random-dependent stopping rule.

## ADR-132: Branches reconstruct isolated production engines

**Decision.** Every branch starts in a fresh evaluation harness, replays its exact prior edges, rebuilds current production continuations/ranking, and submits the selected menu action through the ordinary production verifier. Caller/live state is never exploration state.

## ADR-133: Search identity is independently domain separated

**Decision.** Objective, plan, node, edge, checkpoint, trace, result and repair use independent `agentir.evaluation.search_*` v1 domains. They never substitute for compiler, ranking/model/inference or archive hashes.

## ADR-134: Structural objectives are ordered checked integers

**Decision.** Stage 7A objectives are explicit ordered lexicographic structural vectors. Hardware/timing/provider/future/reference/label/split fields reject. Objective components remain interpretable and no opaque float score is introduced.

## ADR-135: Algorithmic envelope and safety limits are separate

**Decision.** Beam width, semantic depth, children, order and checkpoint cadence enter `search_plan_hash`. Operational graph/engine/request/byte caps and wall-clock samples enter no search identity.

## ADR-136: Duplicate states preserve provenance

**Decision.** Search-local IDs are deterministic `search-node-N`/`search-edge-N`. The first published equal compiler-state observation is the canonical representative; later alternative-parent nodes/edges remain retained and are marked duplicate rather than silently merged.

## ADR-137: Checkpoint and cancellation are unit-boundary deterministic

**Decision.** Checkpoints retain the exact next semantic work cursor and verify every anchor/graph/frontier invariant before execution. Cancellation is cooperative only between parent-expansion units. Advance partitioning does not change semantic trace/result bytes.

## ADR-138: Search recommendations are non-authoritative

**Decision.** Results say selected terminal trajectory, highest-ranked observed terminal under the exact plan, recommended trajectory, or bounded frontier result. They never say globally optimal or automatically publish/select a compiler artifact.

## ADR-139: Evaluation archive v4 retains search provenance

**Decision.** New evaluation saves use v4. Immutable v1/v2/v3 inputs migrate only v1→v2→v3→v4; v3→v4 explicitly records no search history and invents no objective, plan, node, checkpoint, trace or result. Workspace archives remain unrelated v1–v9.

## ADR-140: Stage 7B remains evaluation-only

**Decision.** Measurement cohorts, measured objectives, recommendation lifecycle/work and archive v5 live only in `agentir-policy-eval`. Core retains unchanged measurement-record v1 and workspace archive v9; Stage 7A contracts remain byte-frozen.

## ADR-141: Cohorts contain only verified production records

**Decision.** Cohort creation resolves compiler-assigned IDs/hashes from one production workspace, rehashes records, requires retained offline-valid artifacts, canonicalizes by measurement hash, rejects duplicates, and freezes exact validation/count/aggregation policy.

## ADR-142: Cohort eligibility forbids pooling

**Decision.** Target, compiler build, device fingerprint, runtime, warmups, iterations, input distribution and tensor dimensions must be identical. Cross-device/build/input/config records reject; missing measurements remain typed unavailable values.

## ADR-143: Clients cannot supply measurement data

**Decision.** Evaluation JSONL accepts measurement IDs/hashes only. Timing summaries, device metadata, validation status, arbitrary artifacts/backend source, guards and certificates are absent from request variants and rejected by `deny_unknown_fields`.

## ADR-144: Hardware objectives are terminal-only and separate

**Decision.** Immutable `SearchObjectiveDescriptor` v1 and Stage 7A ordering do not change. `MeasuredObjectiveDescriptor` v1 applies only after search stops to terminal artifacts with eligible cohort records; intermediate nodes receive no latency estimate.

## ADR-145: Aggregation and indifference use checked integers

**Decision.** V1 permits median/p95 record summaries, one-record or lower-median-of-record-summaries aggregation, minimize direction, and checked ppm indifference. Equivalent measurements resolve by artifact hash/node ID without a faster-than claim.

## ADR-146: Measured recommendations are non-authoritative

**Decision.** A recommendation says selected under one descriptor from one cohort, never proven fastest, statistically significant, portable, globally optimal or correctness evidence. It never publishes a live workspace.

## ADR-147: Replay performs no hardware work

**Decision.** Replay repeats ordinary Stage 7A production/ranker reconstruction then cohort validation, terminal eligibility, integer aggregation/ties and recommendation hashing. Benchmark/device/provider/network/training calls remain zero.

## ADR-148: Evaluation archive v5 retains measured provenance

**Decision.** New saves use v5 and `agentir.evaluation.archive.v5\0`. Immutable v1–v4 inputs migrate only v1→v2→v3→v4→v5; v4→v5 verifies first and records `NoMeasuredSearchHistory` without synthetic data.

## ADR-149: Hardware observations remain non-correctness

**Decision.** Cohorts, objectives, timing, recommendations, search and selection cannot advance proof frontiers, legalize compiler IR/artifacts, close obligations, or change any compiler semantic hash.

## ADR-150: Stage 7C remains separately versioned

**Decision.** Continuation-native snapshots, concurrent/new search algorithms, broader surfaces, live acquisition orchestration, energy objectives, prediction/interpolation, training during search and global optimization remain deferred to a separately frozen Stage 7C or later contract.

## ADR-151: Stage 7C acquisition is evaluation-only

**Decision.** Plans, sessions, slots, checkpoints, traces, results and archive-v6 records live only in `agentir-policy-eval`; core/store/compiler semantics and workspace v9 remain unchanged.

## ADR-152: Hardware work is explicit and server owned

**Decision.** Only acquisition start preflight and advance may access WebGPU. Device/build/runtime/validation/timing/measurement metadata, package bytes, ABI and success remain server owned.

## ADR-153: Acquisition order is canonical round robin

**Decision.** V1 sorts exact artifact hashes and visits each once per round. Request order, wall time, paths, limits and progress cannot change plan identity.

## ADR-154: Slots are the checkpoint and cancellation boundary

**Decision.** A slot either atomically publishes one complete production record with session progress or retains a typed failure without a record/sentinel. Cancellation never interrupts a benchmark transaction.

## ADR-155: Recovery claims match the storage boundary

**Decision.** The in-memory staged wrapper atomically assigns measurement store and session progress. Independent filesystem exactly-once recovery is not claimed; unresolved external publication ambiguity is `IndeterminateAfterCrash` and cannot silently rerun.

## ADR-156: Replay performs zero hardware work

**Decision.** Replay has no executor, verifies frozen observations and hashes, and rejects missing/corrupt/duplicate/stale records before publication.

## ADR-157: Stage 7B handoff is separate

**Decision.** Only a complete acquisition result can explicitly invoke the existing cohort eligibility/canonicalization path. Acquisition never starts search or chooses/publishes an artifact.

## ADR-158: Evaluation archive v6 retains acquisition provenance

**Decision.** V5→v6 verifies v5 and adds `NoAcquisitionHistory` without synthetic data. V1–v5 remain immutable inputs; workspace v9 and measurement-record v1 are unchanged.

## ADR-159: Acquisition makes no authority claim

**Decision.** Results are observations, never correctness evidence, proven-fastest/statistical-significance/portability/global-optimality claims or full Stage 7 completion.

## ADR-160: Broader acquisition remains deferred

**Decision.** Concurrency, remote workers, multi-device pooling, prediction/interpolation, energy objectives, raw-sample inference and training during acquisition require later contracts.

## ADR-161: Recovery makes no exactly-once hardware claim

**Decision.** Stage 7D proves no silent automatic rerun and at most one accepted measurement per Stage 7C slot. A physical benchmark may have executed before a crash; the journal never promotes that uncertainty into an exactly-once claim.

## ADR-162: Recovery v1 is single-workspace and single-writer

**Decision.** One recovery journal protects one canonical slot against one production measurement store. Concurrent writers, distributed transactions, remote workers and multi-device pools require a later contract.

## ADR-163: Durable preparation precedes hardware authorization

**Decision.** `prepare` verifies all Stage 7C anchors, snapshots production publications and assigns an attempt ID before any benchmark. Only a separate explicit `execute` operation receives an executor.

## ADR-164: Publication snapshots are server owned

**Decision.** The prepared boundary is the canonical ordered set of existing production measurement IDs and reverified hashes. Clients cannot supply timing/device/build/validation data, record selections, outcomes, execution claims or recovery certificates.

## ADR-165: Reconciliation uses zero/one/multiple semantics

**Decision.** Zero compatible post-boundary publications remains an observed absence; one may be atomically attached to the pending Stage 7C slot; multiple remain typed ambiguous. Incompatible or changed anchors block without hardware work.

## ADR-166: Retry requires a new explicit authorization

**Decision.** Only a latest zero-publication reconciliation permits `authorize_retry`. It creates a new immutable attempt ID and trace event; the prior attempt is never rewritten or silently executed again.

## ADR-167: Indeterminate slots never rerun silently

**Decision.** Crashes before benchmark, after benchmark and after publication all retain an explicit indeterminate recovery state until reconciliation, retry authorization or abandonment. Ordinary Stage 7C resume does not bypass the journal.

## ADR-168: Recovery replay is zero-device

**Decision.** Status, checkpoint, restore, reconciliation, result, replay and archive verification accept no executor. Replay rehashes journals and referenced production records and rejects any non-zero replay hardware-call accounting.

## ADR-169: Stage 7A, 7B and 7C contracts remain immutable

**Decision.** Stage 7D adds independent journal, prepared-slot and reconciliation domains. Existing search, measurement cohort, measured recommendation, acquisition plan/checkpoint/trace/result hashes and ordering semantics are unchanged.

## ADR-170: Workspace v9 and measurement record v1 remain immutable

**Decision.** Recovery is implemented in `agentir-policy-eval` over the existing read/publish store boundary. It does not change workspace archives, `HardwareMeasurementRecord`, compiler semantics, hashes or proof frontiers.

## ADR-171: Evaluation archive v7 retains recovery provenance

**Decision.** V6→v7 first verifies v6 and adds `NoRecoveryHistory` without synthetic records. New saves use v7; the only load chain is v1→v2→v3→v4→v5→v6→v7.

## ADR-172: Broader recovery remains deferred

**Decision.** Concurrency, distribution, cross-device comparison, automatic retry, live artifact publication, prediction/training, statistical significance, energy objectives and new search/ranking algorithms remain out of Stage 7D.

## ADR-173: Integrated campaigns are evaluation-owned composition

**Decision.** Stage 7E lives only in `agentir-policy-eval` and retains exact Stage 7A–7D records. It adds no compiler semantics, proof authority, search algorithm, ranker, or contract substitution.

## ADR-174: Terminal selection is deterministic and timing-blind

**Decision.** V1 uses every distinct proved/offline-valid terminal artifact from the frozen Stage 7A graph, ordered by artifact hash and bounded by an explicit cap. No timing or new policy preselects artifacts.

## ADR-175: Campaign hardware has one explicit boundary

**Decision.** Only `execute_prepared` accepts the server-owned executor. All other campaign operations, including replay and verification, are zero-device. Retry remains an explicit Stage 7D authorization with a new attempt ID; physical exactly-once execution is not claimed.

## ADR-176: Campaign selection never publishes live state

**Decision.** A measured recommendation is non-correctness evaluation data. Campaign finalization records it but cannot publish an artifact or claim performance superiority, portability, significance, or global optimality.

## ADR-177: Evaluation archive v8 adds campaign history only

**Decision.** Workspace archive v9, `HardwareMeasurementRecord` v1, and Stage 1–7D hashes remain immutable. V7→v8 first verifies v7 and adds `NoCampaignHistory`; it invents no older-stage records.

## ADR-178: Full Stage 7 closure requires a controlled device gate

**Status.** Superseded by ADR-180 for project readiness. The implemented device lifecycle remains an immutable optional compatibility surface.

**Decision.** Offline tests and synthetic byte identity are necessary but insufficient. Stage 8 scope remains blocked until a production workspace, WebGPU adapter, at least two compatible terminal artifacts, post-publication crash/restart/reconciliation, final checkpoint, and zero-device replay complete successfully.

## ADR-179: Broader autotuning remains deferred

**Decision.** Stage 7E stays single-workspace/single-writer. Concurrency, distribution, remote workers, multi-device pools, prediction, interpolation, training, energy objectives, statistical inference, automatic retry, and new search/ranking algorithms are later scope.

## ADR-180: Stage 7 readiness uses an offline-only closure gate

**Decision.** The active project strategy does not depend on physical GPU availability. Stage 7 closes when the full offline gate passes: one production-replayed Stage 7A search publishes at least two distinct proved/offline-valid terminal artifacts; canonical materialization feeds only explicitly labelled synthetic acquisition data; Stage 7C–7E lifecycle, recovery, checkpoint, recommendation, replay and evaluation archive checks pass deterministically; replay and every non-execution operation report zero device calls; and the full workspace quality gate passes.

The WebGPU executor, device discovery and physical recovery path remain supported optional compatibility surfaces, but they are not exercised by the default strategy and do not block Stage 8 scope. This readiness change does not alter compiler semantics, proof frontiers, workspace archive v1–v9, evaluation archive v1–v8, `HardwareMeasurementRecord` v1, or any Stage 6/7 hash domain.

Offline closure proves deterministic orchestration and contract integrity only. It does not prove physical execution behavior, performance superiority, portability, statistical significance, exactly-once hardware execution, compiler correctness, or global optimality. Requiring hardware again needs a new explicit ADR and acceptance gate.

## ADR-181: Stage 8A adds a portable scalar CPU artifact and archive v10

**Decision.** Stage 8A introduces the immutable compiler-owned
`cpu_scalar_v1` TargetManifest under a new CPU target hash domain and one
separately domain-separated versioned CPU artifact. A trusted compiler lowers a
proved serial ScheduleIR revision into bounded scalar bytecode for the minimal
one-dimensional f32 elementwise subset. A safe interpreter validates the
package, runtime names, types, dimensions, anchors and checked size/index
arithmetic before executing it. Clients can select retained IDs and provide
runtime inputs, but cannot submit bytecode, bindings, execution plans, hashes,
results, success claims or certificates.

`CpuArtifactEquivalentToSchedule` is compiler-owned structural evidence.
Offline package validation establishes structure; CPU execution results and
counters are non-correctness observations. Runtime limits, inputs, counters,
timings and machine metadata enter no compiler or artifact identity. The CPU
artifact hash is independent from WGSL `artifact_hash` and every Stage 1-7
contract.

CPU packages are persisted in workspace archive v10. V1-v9 remain immutable
legacy inputs; v9 migrates explicitly by adding an empty CPU artifact store and
inventing no package. Replay and archive verification perform zero bytecode
execution. Existing WebGPU BackendIR, WGSL packages, hashes, fixtures and
optional runtime remain unchanged.

Stage 8A deliberately excludes JIT, LLVM, MLIR, native code generation, native
ABI, dynamic libraries, external processes, raw pointers, `unsafe`, threads,
SIMD, GPU work, autotuning and performance ranking. Real CPU timing records and
Stage 7 measurement/recommendation integration require a separate Stage 8B
contract.

## ADR-182: Stage 8B is bounded CPU observation with archive v11

**Decision.** Stage 8B adds `agentir-runtime-cpu` as the sole monotonic-clock and benchmark-orchestration boundary over unchanged, compiler-published `cpu_scalar_v1` packages. Core owns immutable measurement/config/input/host/output hash contracts, structural validation, an independent `CpuMeasurementStore`, atomic events, and zero-execution replay. The client supplies only an artifact ID and exact artifact hash, bounded v1 configuration, and ordinary inputs; runtime-owned timing, host, aggregates, outputs, hashes, bytecode, ABI, proof and success fields are forbidden.

Only `cpu_measurement.acquire` reads the clock or executes bytecode. All other Stage 8B commands and archive operations are zero-execution. Measurements are non-correctness observations and have no ranking, selection, publication, significance, portability, or performance-proof authority. Resource policy is enforced but excluded from every measurement identity.

New saves use workspace archive/snapshot v11. V1–v10 remain immutable legacy inputs; the sole v10→v11 edge adds an empty CPU measurement store without invented history. Stage 1–8A hashes, CPU bytecode/build identity, WebGPU measurement-record v1, and evaluation archives/contracts remain unchanged.

## ADR-183: Stage 8 closes through an offline CPU execution and measurement gate

**Decision.** Stage 8A portable deterministic CPU execution and Stage 8B bounded CPU measurement are complete contracts. Stage 8 closes when one fast offline integration gate reconstructs the production SpecIR→ImplIR→MemoryIR→ScheduleIR→CPU-artifact chain, executes the unchanged package, records one isolated synthetic measurement with explicit clock/execution doubles, structurally checks it, round-trips archive v11, rejects corrupt record/cursor state, and verifies pure v10→v11 migration. Query/check/replay remain capability-free structural paths; replay reuses archived revision timestamps instead of consulting a clock.

Stage 8C is validation and closure only. It adds no correctness or performance authority, persisted state, archive/evaluation version, hash domain, mutation command, target, opcode, lowering, benchmark algorithm, ranking, recommendation, selection, or live publication path. Physical monotonic timing is an optional compatibility observation with no threshold or comparative claim. Synthetic evidence establishes deterministic orchestration, hashing, atomicity, and replay, not speed.

SIMD, threads, native/JIT/AOT code, LLVM/MLIR, reductions, broader types/ranks, CPU/GPU comparison, statistical inference, ranking, search, and autotuning remain future scope and do not block Stage 8 closure. Reopening Stage 8 or granting timing any acceptance authority requires a new ADR.
