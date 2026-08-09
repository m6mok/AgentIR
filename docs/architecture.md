# Architecture

AgentIR has five explicit immutable compiler graph layers: SpecIR states semantics, ImplIR states an exact implementation, MemoryIR states typed physical storage, ScheduleIR states target-checked execution order, and BackendIR states executable typed kernels for one schedule. Artifact WGSL remains derived output, never canonical input. Stage 6A/6B is a separate non-correctness evaluation and ranking layer above the production protocol. Core snapshot/replay remains I/O-free; only `agentir-store` reads or writes workspace archive v9.

## Data flow

```text
JSONL client
  ↓ Request
agentir-protocol ── stateful workspace registry and response envelope
  ↓ ActionIR
agentir-core ────── budget → resolve → fact-aware infer → verify → atomic commit → Revision
  ↓ frozen SpecIR
semantic codec ─── alpha-normalize → reachable DAG → spec_hash
  ↓ identity lowering
ImplIR ──────────── separate typed graph → verify → impl_hash
  ↓ exact rewrites or verifier-gated proposals
CandidateForest ─── atomic revisions → proof debt/validation/guard → candidate_hash v1/v2/v3
  ↓ proved exact anchor / selected member
EqualityStore ───── whole-program nodes → trusted edges → bounded saturation → equality_hash
  ↓ explicit materialized exact candidate
MemoryIR ────────── typed regions → alias/lifetime proof → exact reuse/fallback → memory_hash
  ↓ immutable TargetManifest
ScheduleIR ──────── domains → transforms → resource proof → schedule_hash
  ↓ webgpu_wgsl_v1 lowering
BackendIR ───────── typed kernels → ABI → dispatch proof → backend_hash
  ↓ deterministic emission + offline validation
WGSL artifact ───── manifest + exact module bytes → artifact_hash
  ↓ optional, confidence only
wgpu runtime ────── device fingerprint → execution/measurement
  ↓ SpecIR + ImplIR + MemoryIR
agentir-eval ────── deterministic CPU semantic/physical oracle and memory trace

agentir-core snapshot/all graph, artifact and measurement event logs
  ↓
agentir-store ───── version sniff → source checksum → migrate → replay
  ↓ save/migrate
archive v9 ─────── checksum → temp write + sync → atomic rename

immutable corpus + policy descriptor
  ↓ exact observation / bounded decision
agentir-policy-eval ─ production outcome → ranking/learning → isolated bounded search
  ↓ separate format
evaluation archive v5 (v1→v2→v3→v4→v5 migration; never workspace archive v9)
```

The dependency direction is one-way: `core` knows nothing about JSONL sessions, policy evaluation, learned models, search plans/frontiers, transcripts or filesystems; the reference evaluator and store depend on `core`; `protocol` composes production components; `agentir-policy-eval` invokes that production surface and owns offline ranking, learning and bounded search; both CLIs only stream lines.

## Stage 7A search boundary

Stage 7A reconstructs each explored trajectory in a fresh `EvaluationHarness`, rebuilds exact choice sets, reruns the retained Stage 6B/6C ranker, and submits each selected menu action through `agentir-protocol::Engine`. Search-local graphs, checkpoints and results are evaluation records only. No live workspace is exploration state and no search dependency flows into core.

## Stage 7B measured-search boundary

Stage 7B leaves Stage 7A byte contracts unchanged and post-processes only terminal artifacts using frozen, production-hash-verified measurement cohorts. Cohort/objective/recommendation/archive-v5 types remain in `agentir-policy-eval`; core retains the unchanged measurement-record v1 and workspace archive v9. Search and replay never acquire hardware measurements.

## Candidate boundary

ImplIR is not an attribute-bearing SpecIR. Candidate identities, revisions, evidence and rewrite provenance evolve independently while the candidate retains one immutable frozen `spec_hash`. The candidate allocator is separate so adding Stage 2A cannot change historical SpecIR ID allocation or `content_hash` replay.

Exact rules are registry-owned. Each accepted action verifies side conditions, stages the graph, re-runs the ImplIR verifier and appends a certificate whose hashes compose from identity lowering to current ImplIR. Stage 2B proposals cross a stricter boundary: the core normalizes and type-checks them, but records unproved meaning as ordered debt. Only compiler validation advances the proof frontier. Differential evaluation is confidence evidence only.

Stage 2C shares that same production matcher/transform with a persistent exact equality space. Nodes are verified whole ImplIR programs hash-consed by `impl_hash`; edges are positive compiler-owned proofs. Saturation is deterministic and bounded, not an e-graph extractor or ranking/search layer. A selected node becomes a candidate only through explicit materialization using ordinary CandidateForest transactions.

Conditional execution exists at candidate level, not inside ImplIR v1. The sole guard evaluates an i32 dependency cone and lazily selects either the speculative primary or an immutable proved fallback. See [stage-2b-scope.md](stage-2b-scope.md).

## Canonical program

`Program` stores dimensions, operations, SSA values, parameters, constants, outputs, holes, constraints, obligations and numeric contract. `BTreeMap` makes serialization order deterministic. Operations additionally have explicit topological insertion order; the interpreter evaluates recursively and detects cycles because filling a hole may introduce a forward reference.

Every operation has an opcode, operand IDs, result IDs, attributes, an optional region, provenance and inferred result types. Stage 1 emits one result but uses vectors so multi-result evolution does not require replacing the operation model.

## Atomic transaction

`Workspace::apply` reads one immutable base revision, clones its `Program` and ID allocator, and applies actions to those staged values. Reference resolution, region closure checks, type inference and shape classification happen before publication. On any error, staged data is dropped. On success, the core allocates a transaction ID and revision ID, computes canonical bytes/hash, inserts one child revision and moves head.

Ordinary writes require the current head. Deliberate search branching uses `allow_branch` or `revision.fork`.

## Holes and obligations

A hole is both a synthesis target and a typed placeholder value. `fill_hole` checks type and shape before attaching a value. An open hole has an open `HoleFilled` obligation and blocks freeze and evaluation.

Unknown symbolic equality is not treated as proof. The operation is `conditional` and receives a structured `ShapeCompatible` obligation. `ConstraintFacts` rechecks its exact left/right types when a new equality arrives; proved obligations close, unrelated facts leave them open, and contradictions reject the transaction atomically.

## Semantics versions and budgets

New SpecIR events use core semantics v2. Candidate equality discharge/materialization uses candidate semantics v3 while legacy candidate events retain v1/v2. Equality events use equality semantics v1 and record their candidate-event dependency cursor. Mixed histories replay each event under its own semantics; archive version remains a separate on-disk codec axis.

`ResourceLimits` is runtime policy outside canonical state. Transaction sizes are projected before graph cloning and ID allocation; evaluator output is projected before tensor results; archive counts are checked before replay. Archive replay uses hard safety caps rather than configurable interactive limits.

## Continuation frames

The core derives frames from the same verified program used by free transactions. A frame gives an opcode enum plus dependent operand-domain queries; it never expands the Cartesian product. `menu` disables escape, while `hybrid` adds soft ranking and a verifier-gated speculative escape.

## Exact state and semantic canonical form

`Program` is serialized to compact deterministic JSON and hashed as `content_hash`. It intentionally includes compiler IDs, obligations and provenance; revision timestamp remains outside it. A separate versioned semantic codec traverses only the output-reachable typed graph, alpha-normalizes dimensions and region locals, and produces `spec_hash` for complete frozen revisions. See [semantic-canonicalization.md](semantic-canonicalization.md) and ADR-003/ADR-012 in [DECISIONS.md](../DECISIONS.md).

## Persistence boundary

The core snapshot contains every graph store plus target, backend, artifact and measurement histories. Loading never trusts serialized graphs directly: `agentir-store` checks the exact source codec/hash, migrates through every explicit edge, verifies dependency cursors, graphs, certificates, package bytes and independent hashes, then publishes the complete workspace. See [persistence.md](persistence.md).
# Stage 4 scheduling layer

One proved MemoryIR revision plus one immutable TargetManifest anchors an independent SchedulePlan DAG. The core derives domains and dependencies, verifies transforms and memory compatibility, simulates target resources, and emits compiler-owned evidence before publication. Target and schedule events replay after their candidate/equality/memory dependencies. Backend lowering remains outside every Stage 4 crate boundary.

# Stage 5 backend and runtime boundary

`agentir-core` owns BackendIR, hashes, certificates, lifecycle, events and package models without depending on Naga or wgpu. `agentir-backend-wgsl` is the trusted lowering/emission adapter and uses Naga for mandatory offline validation. `agentir-runtime-wgpu` is optional and has no correctness authority. `agentir-protocol` composes these components but never accepts source or proof payloads from clients.
