# Architecture

AgentIR has five explicit immutable compiler graph layers: SpecIR states semantics, ImplIR states an exact implementation, MemoryIR states typed physical storage, ScheduleIR states target-checked execution order, and BackendIR states executable typed GPU kernels for one schedule. WGSL and portable CPU bytecode remain derived artifacts, never canonical program input. Stage 6A/6B is a separate non-correctness evaluation and ranking layer above the production protocol. Core snapshot/replay remains I/O-free; only `agentir-store` reads or writes workspace archive v11.

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
archive v11 ────── checksum → temp write + sync → atomic rename

immutable corpus + policy descriptor
  ↓ exact observation / bounded decision
agentir-policy-eval ─ production outcome → ranking/learning → isolated bounded search
  ↓ separate format
evaluation archive v8 (v1→v2→v3→v4→v5→v6→v7→v8 migration; never workspace archive v11)
```

Stage 8B adds a side boundary from compiler-published CPU artifacts to `agentir-runtime-cpu`, which performs bounded real interpreter execution and monotonic timing. It returns a runtime-owned draft to core for atomic publication in `CpuMeasurementStore`. Only acquisition crosses the clock/execution boundary; structural queries and archive replay do not.

Stage 8C closes this architecture without adding a layer or state. One offline integration gate checks the complete proved compiler chain, unchanged package execution, isolated synthetic measurement orchestration, exact artifact/hash stability, archive v11 replay, corruption rejection, and pure v10→v11 migration. A separate real-clock smoke observation has no threshold or correctness/performance authority.

Stage 9B adds a non-persistent side boundary from one retained Stage 8A package to `agentir-runtime-native-cpu`. The safe parent validates the package, inputs, shapes, checked work and server resource policy, then launches the current server executable in a hidden mode selected before JSONL processing. The fresh child clears inherited environment configuration, independently validates the package, performs fixed-setting Cranelift lowering and one audited native call, emits one bounded response, and exits. The parent validates process exit, stderr/framing, artifact/runtime/execution identities, output coverage/shapes/finiteness and both native observation hashes. `agentir-protocol` depends only on the safe parent runtime and has no dependency on the Cranelift-owning worker crate. Machine code is ephemeral worker-local state; the process boundary is crash containment, not a security sandbox.

The dependency direction is one-way: `core` knows nothing about JSONL sessions, policy evaluation, learned models, search plans/frontiers, transcripts or filesystems; the reference evaluator and store depend on `core`; `protocol` composes production components and the safe native parent runtime; the Cranelift worker depends on that shared safe wire contract but never flows back into `protocol`; `agentir-policy-eval` invokes the production surface and owns offline ranking, learning and bounded search; both CLIs only stream lines.

## Stage 7A search boundary

Stage 7A reconstructs each explored trajectory in a fresh `EvaluationHarness`, rebuilds exact choice sets, reruns the retained Stage 6B/6C ranker, and submits each selected menu action through `agentir-protocol::Engine`. Search-local graphs, checkpoints and results are evaluation records only. No live workspace is exploration state and no search dependency flows into core.

## Stage 7B measured-search boundary

Stage 7B leaves Stage 7A byte contracts unchanged and post-processes only terminal artifacts using frozen, production-hash-verified measurement cohorts. Cohort/objective/recommendation/archive-v5 types remain in `agentir-policy-eval`; core retains the unchanged measurement-record v1. Search and replay never acquire hardware measurements.

## Stage 7C acquisition boundary

Stage 7C adds only evaluation-owned orchestration above retained Stage 5 artifacts and measurements. Explicit start performs server-owned preflight; explicit advance executes complete round-robin slots and atomically publishes a production record with session progress. Checkpoint/resume/cancel occur between slots. Replay/archive load have no executor and zero device calls. Stage 7A/7B hashes, core, store, workspace v9 and measurement-record v1 are unchanged.

## Stage 7D recovery boundary

Stage 7D wraps one pending Stage 7C slot in an evaluation-owned durable journal.
Prepare snapshots exact production measurement IDs/hashes before hardware is
authorized. Only explicit execute receives an executor. After uncertainty,
reconciliation observes the server-owned production store without hardware and
classifies zero, one, multiple, incompatible, or changed-anchor publications.
Zero requires separate retry authorization; one can advance the existing Stage
7C session; multiple remains ambiguous. V1 is single-workspace/single-writer and
makes no distributed-atomicity or exactly-once execution claim.

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

# Stage 7E integrated evaluation campaign

`agentir-policy-eval` alone owns the Stage 7E campaign graph. It retains exact Stage 7A search, Stage 7C acquisition, Stage 7D recovery, and Stage 7B cohort/recommendation records without copying their semantics. The executor is available only to explicit prepared-slot execution. Evaluation archive v8 persists campaign history separately from workspace archive v11.

Stage 8A adds no sixth canonical graph. `cpu_scalar_v1` selects a compiler-owned serial target contract; trusted lowering publishes a content-addressed portable scalar package directly from one proved ScheduleIR revision. `agentir-backend-cpu` owns lowering and the safe interpreter, while core owns package identity, certificates, persistence, replay, and atomic publication. Execution validates inputs and package structure before interpretation, returns deterministic work counters, performs no device discovery, and never mutates correctness state.
