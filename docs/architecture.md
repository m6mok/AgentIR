# Architecture

## Data flow

```text
JSONL client
  ↓ Request
agentir-protocol ── stateful workspace registry and response envelope
  ↓ ActionIR
agentir-core ────── resolve → infer → verify → atomic commit → Revision
  ↓ frozen SpecIR
semantic codec ─── alpha-normalize → reachable DAG → spec_hash
  ↓ frozen Program
agentir-eval ────── deterministic CPU semantic oracle

agentir-core snapshot/event log
  ↓
agentir-store ───── version sniff → source checksum → migrate → replay
  ↓ save/migrate
archive v2 ─────── checksum → temp write + sync → atomic rename
```

The dependency direction is one-way: `core` knows nothing about JSONL sessions, evaluation input encoding or filesystems; `eval` and `store` depend on `core`; `protocol` composes them; `cli` only streams lines.

## Canonical program

`Program` stores dimensions, operations, SSA values, parameters, constants, outputs, holes, constraints, obligations and numeric contract. `BTreeMap` makes serialization order deterministic. Operations additionally have explicit topological insertion order; the interpreter evaluates recursively and detects cycles because filling a hole may introduce a forward reference.

Every operation has an opcode, operand IDs, result IDs, attributes, an optional region, provenance and inferred result types. Stage 1 emits one result but uses vectors so multi-result evolution does not require replacing the operation model.

## Atomic transaction

`Workspace::apply` reads one immutable base revision, clones its `Program` and ID allocator, and applies actions to those staged values. Reference resolution, region closure checks, type inference and shape classification happen before publication. On any error, staged data is dropped. On success, the core allocates a transaction ID and revision ID, computes canonical bytes/hash, inserts one child revision and moves head.

Ordinary writes require the current head. Deliberate search branching uses `allow_branch` or `revision.fork`.

## Holes and obligations

A hole is both a synthesis target and a typed placeholder value. `fill_hole` checks type and shape before attaching a value. An open hole has an open `HoleFilled` obligation and blocks freeze and evaluation.

Unknown symbolic equality is not treated as proof. The operation is `conditional` and receives a `ShapeCompatible` obligation. Contradictory static shapes are rejected.

## Continuation frames

The core derives frames from the same verified program used by free transactions. A frame gives an opcode enum plus dependent operand-domain queries; it never expands the Cartesian product. `menu` disables escape, while `hybrid` adds soft ranking and a verifier-gated speculative escape.

## Exact state and semantic canonical form

`Program` is serialized to compact deterministic JSON and hashed as `content_hash`. It intentionally includes compiler IDs, obligations and provenance; revision timestamp remains outside it. A separate versioned semantic codec traverses only the output-reachable typed graph, alpha-normalizes dimensions and region locals, and produces `spec_hash` for complete frozen revisions. See [semantic-canonicalization.md](semantic-canonicalization.md) and ADR-003/ADR-012 in [DECISIONS.md](../DECISIONS.md).

## Persistence boundary

The core snapshot contains the complete revision DAG, allocator state and ordered transaction/fork events. Loading never trusts a serialized graph directly: `agentir-store` reads bounded bytes, selects the source codec, checks that version's envelope, runs the explicit migration registry, then core replay rebuilds history and verifies content/status/spec hashes. Only a fully verified workspace is inserted into the protocol engine. See [persistence.md](persistence.md).
