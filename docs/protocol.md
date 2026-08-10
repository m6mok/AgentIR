# JSONL protocol

Stage 6A policy evaluation uses a separate bounded JSONL frontend documented in [external-agent-protocol.md](external-agent-protocol.md). Its `evaluation.*` commands orchestrate immutable tasks and recorded production requests; they do not change this compiler command enum or workspace archive semantics. One physical input line still yields exactly one structured response.

Stage 6B adds ranking policy list/query, exact choice-set query, `evaluation.episode.rank`, trace query, ranking aggregate and ranking comparison commands. Scores are checked fixed-point integers; clients cannot supply choice IDs, legality, outcomes, proof, success, or ranking hashes. See [external-ranker-protocol.md](external-ranker-protocol.md).

## MemoryIR commands

Stage 3 adds `memory.create`, `memory.query`, `memory.check`, `memory.apply`, `memory.fork`, `memory.seal`, `memory.evaluate`, `memory.alias_query`, `memory.buffer_query`, and `memory.continuation`. Mutations name an explicit base revision and exact `memory_hash`; `memory.apply` also supplies the immutable `impl_hash` and a bounded list of desired compiler-verified actions. Proof payloads and guard expressions are unknown fields and are rejected.

`memory.continuation` returns individual result/input choices, static applicability or the failed stable condition, fresh fallback availability, the sole guard profile, layout/address-space domains, and expected hashes. `memory.evaluate` is read-only and returns semantic outputs, actual guard outcomes, trace codec version, and a deterministic high-level physical trace.

The `agentir` CLI accepts one JSON object per stdin line and emits exactly one response line. Command names are coarse-grained; opcodes are data inside transactions, not separate tools.

The CLI uses a bounded byte reader rather than `BufRead::lines`: request bytes and JSON nesting are checked before deserialization, invalid UTF-8 is structured, and an oversized physical line is discarded through its newline before the next request is processed. `request_id` is preserved from a safe retained prefix when possible, otherwise it is `unknown`.

## Response envelope

Success:

```json
{"ok":true,"request_id":"q1","result":{},"diagnostics":[]}
```

Failure:

```json
{"ok":false,"request_id":"q1","error":{"code":"TYPE_MISMATCH","message":"operand types differ","expected":"f32","actual":"bool"},"diagnostics":[]}
```

Clients should branch on stable `code`, not message text.

## References

- `$name` — temporary binding visible only later in the same transaction;
- `vN`, `hN`, `dN` — persistent compiler-assigned IDs;
- `@vN` — persistent value in short-reference form;
- `@name` — current parameter or output name.
- `@N` — one-based index in the revision's deterministic live-value table.

Temporary bindings never survive commit. A successful response maps them to persistent IDs.
Inside a type string, `[$N]` is accepted as codec sugar and immediately canonicalized to `[N]`; the named dimension must already have been declared by an earlier action.

## Build transaction

```json
{
  "command": "spec.apply",
  "request_id": "build",
  "workspace": "w1",
  "base_revision": "r0",
  "actions": [
    {"kind":"create_parameter","bind":"$x","name":"x","type":"f32"},
    {"kind":"create_constant","bind":"$one","type":"f32","value":1.0},
    {"kind":"create_op","bind":"$out","opcode":"add","operands":["$x","$one"]},
    {"kind":"set_output","name":"out","value":"$out"}
  ]
}
```

An optional `allow_branch: true` explicitly permits a transaction based on a non-head revision. Without it, the response is `BASE_REVISION_CONFLICT`.

## Regions

`map`, `zip_map` and `reduce` accept an inline `region` with typed `arguments`, an explicit `captures` list, local `operations`, and `yield_value`. Region op bindings also start with `$`, but use a namespace local to the region.

```json
{
  "arguments":[{"name":"xi","type":"f32"}],
  "captures":["$scale"],
  "operations":[
    {"bind":"$product","opcode":"mul","operands":["$scale","xi"]}
  ],
  "yield_value":"$product"
}
```

## Commands

- `workspace.open`: optional `workspace`; returns root `r0`.
- `workspace.save`: workspace and destination `path`; writes an atomic archive v7.
- `workspace.load`: archive v1/v2/v3/v4/v5/v6/v7 `path` and optional `replace`; verifies, migrates and replays before inserting.
- `workspace.verify_archive`: verifies checksum and replay without retaining the workspace.
- `workspace.migrate_archive`: verifies `source_path`, migrates in memory and atomically writes `destination_path`; existing destinations require `overwrite: true`.
- `spec.apply` and `transaction.apply`: workspace, base revision, actions and optional client transaction ID.
- `spec.check`: optional revision, default head.
- `spec.freeze`: base revision; commits `freeze_spec` as a new revision.
- `program.query`: optional revision and `view: summary | canonical | semantic_canonical`.
- `program.evaluate`: optional revision and exact parameter-name `inputs`.
- `revision.fork`: `base_revision`.
- `revision.diff`: `from` and `to`.
- `continuation.get`: revision, hole and `mode: free | menu | hybrid`.
- `candidate.create`: frozen `spec_revision` and optional relation (exact equivalence only).
- `candidate.query` / `candidate.check`: read or fully verify one candidate revision.
- `candidate.apply`: candidate, explicit `base_candidate_revision`, and compiler-known rewrite actions.
- `candidate.propose`: one typed replacement fragment, target, expected hash and explicit speculative opt-in.
- `candidate.proposal_query`: reads normalized proposal provenance by `proposal` ID.
- `candidate.translation_check`: runs the trusted validator on one ordered proposal obligation.
- `candidate.evaluate`: evaluates exact/speculative/guarded candidate-level semantics with lazy fallback.
- `candidate.fork`: new branch identity from one immutable candidate revision.
- `candidate.validate`: fixed seed and bounded cases; creates confidence evidence only.
- `candidate.seal`: seals only a fully proved exact or verified guarded candidate.
- `candidate.continuation`: separate trusted rewrite matches and one bounded speculative escape schema.
- `equality.create`: creates a root-only space from an explicit fully proved unconditional candidate revision.
- `equality.query`: reads one immutable equality revision and exact state hash.
- `equality.expand` / `equality.saturate`: require explicit base revision, expected hash and positive fuel; publish one atomic equality revision.
- `equality.explain`: rebuilds the canonical trusted root-to-member path.
- `equality.evaluate`: evaluates one member as a semantic oracle; it never proves equality.
- `equality.materialize`: explicitly forks the anchor and replays the selected proof path through ordinary candidate rewrites.
- `equality.continuation`: returns bounded deterministic production matches without mutation.
- `candidate.equality_check`: discharges the next matching debt item from a core-built equality path.

The complete SAXPY command sequence is [examples/saxpy.jsonl](../examples/saxpy.jsonl).

## Save and resume

Save the current workspace:

```json
{"command":"workspace.save","request_id":"save","workspace":"w1","path":"/tmp/saxpy.agentir.json"}
```

In a fresh CLI process, restore it:

```json
{"command":"workspace.load","request_id":"load","path":"/tmp/saxpy.agentir.json"}
```

The result contains archive metadata and a replay report. `replace` defaults to `false`, so an archive cannot silently overwrite an already open workspace with the same ID.

Load responses also contain a migration report. V7 reports `workspace_archive_v7_noop`; v6 reports `workspace_archive_v6_to_v7`; older sources report the explicit suffix of the v1 → v2 → v3 → v4 → v5 → v6 → v7 chain.

## Candidate rewrite

```json
{"command":"candidate.apply","request_id":"rw","workspace":"w1","candidate":"c1","base_candidate_revision":"cr1","actions":[{"kind":"apply_known_rewrite","rule":"fold_defined_scalar_constants","target":"iop3","expected_before_impl_hash":"..."}]}
```

Stable rule IDs are `prune_unreachable_impl_nodes`, `eliminate_noop_cast` and `fold_defined_scalar_constants`. The compiler owns matching, side conditions, transformation and certificates; clients cannot submit trusted proof certificates.

## Speculative proposal

```json
{"command":"candidate.propose","request_id":"p","workspace":"w1","candidate":"c1","base_candidate_revision":"cr1","target":"iop3","replacement":{"inputs":[{"bind":"$x","value":"iv1"},{"bind":"$y","value":"iv2"}],"operations":[{"bind":"$r","opcode":"sub","operands":["$x","$y"]}],"result":{"value":"$r"}},"expected_before_impl_hash":"...","allow_speculative":true}
```

Proposal-local bindings are the only accepted names for new values; persistent IDs are compiler outputs. Boundary order and yield type are exact. `claimed_rule`, when present, is untrusted provenance. Unknown or conditional work without opt-in returns `SPECULATIVE_OPT_IN_REQUIRED` atomically. Positive `candidate.validate` results leave proof debt open; call `candidate.translation_check` for compiler-owned proof recognition.

## Semantic query

Summary returns history-sensitive `content_hash` plus `spec_hash` and `semantic_canonical_version` when the revision is complete and frozen. The semantic view recomputes and validates the cache:

```json
{"command":"program.query","request_id":"semantic","workspace":"w1","revision":"r2","view":"semantic_canonical"}
```

Its result contains `semantic_canonical_version`, `canonical`, `canonical_byte_length` and `spec_hash`. Draft or incomplete revisions fail with `SPEC_NOT_COMPLETE`; canonicalizer internal failures use `CANONICALIZATION_FAILED`.

## Migrate archive

```json
{
  "command": "workspace.migrate_archive",
  "request_id": "m1",
  "source_path": "/project/legacy.agentir.json",
  "destination_path": "/project/current.agentir.json",
  "overwrite": false
}
```

The command is intentionally coarse-grained and is not a general filesystem API. It emits one normal response envelope containing `MigrationReport`.

## Codec and resource policy

Top-level requests, ActionIR variants, transactions and inline-region objects reject duplicate or unknown fields. Empty lines, malformed numbers, over-deep JSON, invalid UTF-8 and long/large requests each produce one normal failure envelope. Stable Stage 1.2 errors include:

- `INVALID_CONSTRAINT` for undeclared symbols or unsupported dimension declarations;
- `CONSTRAINT_CONTRADICTION` for a proven fact/obligation conflict;
- `RESOURCE_LIMIT_EXCEEDED` with `resource`, `configured_limit`, `attempted`, `context` and a repair recommendation.

Candidate failures additionally use `PROPOSAL_NOT_FOUND`, `INVALID_PROPOSAL`, `SPECULATIVE_OPT_IN_REQUIRED`, `PROOF_DEBT_LIMIT_EXCEEDED`, `TRANSLATION_UNSUPPORTED`, `OBLIGATION_REFUTED`, `GUARD_INVALID`, `FALLBACK_INVALID`, `FALLBACK_CYCLE` and `CANDIDATE_HAS_PROOF_DEBT`. Structured details identify the proposal/target/obligation, expected and actual hashes, failed side condition and deterministic repair where applicable.

Equality failures use stable codes for missing spaces/revisions/nodes, stale equality bases/hashes, unproved or guarded anchors, invalid proof edges/paths, debt endpoint mismatches and materialization failures. Clients supply only identities, bases, hashes, fuel and selected nodes; no request accepts a trusted edge, side condition or correctness certificate.

Resource limits are policy, not SpecIR. Archive replay uses hard safety caps; normal protocol work uses configurable interactive defaults documented in [resource-budgets.md](resource-budgets.md).
# Stage 4 commands

`target.list`, `target.create`, `target.query`, and `target.check` expose immutable compiler profiles. `schedule.create`, `schedule.query`, `schedule.check`, `schedule.apply`, `schedule.fork`, `schedule.seal`, `schedule.evaluate`, `schedule.resource_query`, `schedule.axis_query`, `schedule.legality_query`, and `schedule.continuation` operate on exact ScheduleIR revisions. Apply requests include an explicit base plus expected schedule, memory, and target hashes. Action objects accept only transform choices; unknown proof, guard, capability, or certificate fields are rejected. The CLI still emits exactly one JSON response per input line.

# Stage 5 commands

`backend.lower`, `backend.query`, `backend.check`, `backend.continuation`, `backend.fork` and `backend.seal` operate on immutable BackendIR plans. `artifact.emit`, `artifact.list`, `artifact.query`, `artifact.check` and `artifact.reference_evaluate` are GPU-independent. `artifact.execute`, `device.list`, `device.query`, and the `benchmark.start/status/cancel/query` family are optional device paths.

`cpu_artifact.emit` accepts only a workspace, schedule plan/revision, and exact expected `schedule_hash`; lowering, bytecode, ABI, bounds checks, IDs, hashes, and certificates remain compiler-owned. `cpu_artifact.list`, `cpu_artifact.query`, and `cpu_artifact.check` are zero-execution structural operations. `cpu_artifact.execute` requires the exact expected `cpu_artifact_hash` plus named JSON inputs and returns named outputs with deterministic work counters. It performs no GPU discovery, timing, benchmarking, proof advancement, or workspace mutation. Unknown client fields such as bytecode, bindings, guards, or certificates are rejected.

Mutations require explicit source revisions and expected schedule/backend/artifact hashes. Requests accept only IDs, stable enums, runtime inputs and bounded benchmark configuration. Unknown WGSL, BackendIR nodes, bindings, dispatch expressions, guards, target capabilities, certificates, fingerprints or measurement results are rejected by `deny_unknown_fields`.
