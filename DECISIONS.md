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

## ADR-003: Canonical serialization and hash

**Decision.** Canonical bytes are compact `serde_json` over deterministic collections, followed by SHA-256. Float constants are stored as exact lowercase IEEE-754 bit strings. Timestamp metadata lives on `Revision`, outside the hashed `Program`.

**Limitation.** Action provenance and compiler-assigned IDs currently participate in the program hash. Re-serializing one revision is stable, but semantically equivalent graphs built with different action/ID histories are not guaranteed to share a hash. A future canonical renumbering pass may remove this limitation.

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

**Decision.** `agentir-core` exposes an I/O-free `WorkspaceSnapshot` containing schema version, immutable revisions, allocator state and an ordered `WorkspaceEvent` log. `agentir-store` wraps it in `agentir.workspace` archive format version 1, hashes the deterministic archive body with SHA-256, writes a same-directory temporary file, calls `sync_all`, and atomically renames it into place.

Loading is deliberately expensive and defensive: verify the archive hash, replay every transaction/fork through the normal compiler core, reproduce compiler IDs and revision hashes, recompute every archived program hash and status summary, then publish the restored workspace. Timestamps are restored metadata and are not replay-equivalence inputs. Ephemeral continuation counters are persisted but excluded from graph-event equivalence.

**Alternatives.** Serializing only the latest graph would resume quickly but could not establish provenance. Replaying only a log would lose original timestamp metadata and make random-access reads expensive. A SQLite/RocksDB workspace database is premature until the archive schema and query workload stabilize.

**Limitations.** Version 1 has no migration framework, process locking, directory `fsync`, compression, incremental snapshots or encryption. The local CLI accepts explicit filesystem paths and remains unsuitable as a multi-tenant server boundary.
