# Architecture

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
CandidateForest ─── atomic revisions → proof debt/validation/guard → candidate_hash v1/v2
  ↓ SpecIR + ImplIR
agentir-eval ────── deterministic CPU semantic/differential oracle

agentir-core snapshot/SpecIR+candidate event logs
  ↓
agentir-store ───── version sniff → source checksum → migrate → replay
  ↓ save/migrate
archive v5 ─────── checksum → temp write + sync → atomic rename
```

The dependency direction is one-way: `core` knows nothing about JSONL sessions, evaluation input encoding or filesystems; `eval` and `store` depend on `core`; `protocol` composes them; `cli` only streams lines.

## Candidate boundary

ImplIR is not an attribute-bearing SpecIR. Candidate identities, revisions, evidence and rewrite provenance evolve independently while the candidate retains one immutable frozen `spec_hash`. The candidate allocator is separate so adding Stage 2A cannot change historical SpecIR ID allocation or `content_hash` replay.

Exact rules are registry-owned. Each accepted action verifies side conditions, stages the graph, re-runs the ImplIR verifier and appends a certificate whose hashes compose from identity lowering to current ImplIR. Stage 2B proposals cross a stricter boundary: the core normalizes and type-checks them, but records unproved meaning as ordered debt. Only compiler validation advances the proof frontier. Differential evaluation is confidence evidence only.

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

New SpecIR events use core semantics v2. Candidate proposal/validation events use candidate semantics v2 while legacy exact candidate events remain v1. Mixed histories replay each event under its own semantics; archive version remains a separate on-disk codec axis.

`ResourceLimits` is runtime policy outside canonical state. Transaction sizes are projected before graph cloning and ID allocation; evaluator output is projected before tensor results; archive counts are checked before replay. Archive replay uses hard safety caps rather than configurable interactive limits.

## Continuation frames

The core derives frames from the same verified program used by free transactions. A frame gives an opcode enum plus dependent operand-domain queries; it never expands the Cartesian product. `menu` disables escape, while `hybrid` adds soft ranking and a verifier-gated speculative escape.

## Exact state and semantic canonical form

`Program` is serialized to compact deterministic JSON and hashed as `content_hash`. It intentionally includes compiler IDs, obligations and provenance; revision timestamp remains outside it. A separate versioned semantic codec traverses only the output-reachable typed graph, alpha-normalizes dimensions and region locals, and produces `spec_hash` for complete frozen revisions. See [semantic-canonicalization.md](semantic-canonicalization.md) and ADR-003/ADR-012 in [DECISIONS.md](../DECISIONS.md).

## Persistence boundary

The core snapshot contains the complete SpecIR revision DAG/allocator/events plus CandidateForest, proposals, proof debt, candidate allocator, EvidenceIR and candidate events. Loading never trusts serialized graphs directly: `agentir-store` checks the exact source codec/hash, migrates, replays SpecIR, verifies its hashes, then replays candidate semantics and verifies proposal/ImplIR/candidate/frontier/debt/guard/evidence chains. Only the complete verified workspace is published. See [persistence.md](persistence.md).
