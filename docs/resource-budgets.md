# Resource budgets

`ResourceLimits`, `ResourceKind`, `ResourceUsage` and `BudgetCheck` provide one structured policy across core, evaluator, store, protocol and CLI. Limits are excluded from `Program`, snapshots and every hash identity.

## Interactive defaults and hard caps

| Resource | Interactive default | Hard-safety profile |
| --- | ---: | ---: |
| JSONL request bytes | 1 MiB | 8 MiB |
| actions per transaction | 4,096 | 100,000 |
| dimensions per program | 4,096 | 100,000 |
| operations per program | 100,000 | 1,000,000 |
| values per program | 120,000 | 1,200,000 |
| holes per program | 10,000 | 100,000 |
| obligations per program | 100,000 | 1,000,000 |
| constraints per program | 20,000 | 200,000 |
| region arguments | 64 | 1,024 |
| region operations | 1,024 | 100,000 |
| operands per operation | 64 | 4,096 |
| attribute JSON bytes | 256 KiB | 4 MiB |
| JSON nesting depth | 128 | 256 |
| outputs | 1,024 | 100,000 |
| archive bytes | 64 MiB | 64 MiB |
| revisions/events per archive | 100,000 / 100,000 | 250,000 / 250,000 |
| actions replayed per archive | 1,000,000 | 5,000,000 |
| canonical output bytes | 64 MiB | 128 MiB |
| elements per evaluation tensor | 10,000,000 | 100,000,000 |
| total evaluation elements | 50,000,000 | 500,000,000 |
| generated property/fuzz case size | 10,000 | 100,000 |
| candidates / branches per workspace | 10,000 / 10,000 | 100,000 / 100,000 |
| candidate revisions/events | 100,000 / 100,000 | 250,000 / 250,000 |
| candidate actions per transaction | 1,024 | 100,000 |
| ImplIR operations/values | 100,000 / 120,000 | 1,000,000 / 1,200,000 |
| rewrite matches/steps | 10,000 / 10,000 | 100,000 / 100,000 |
| evidence records/bytes | 100,000 / 64 MiB | 1,000,000 / 128 MiB |
| open equivalence obligations | 10,000 | 100,000 |
| differential cases/elements | 256 / 1,000,000 | 10,000 / 100,000,000 |
| candidate canonical bytes | 64 MiB | 128 MiB |
| candidate canonical-v2 bytes | 64 MiB | 128 MiB |
| generated candidate case size | 10,000 | 100,000 |
| proposals per workspace/candidate | 100,000 / 1,024 | 1,000,000 / 100,000 |
| proposal actions/operations/values | 1,024 / 1,024 / 1,024 | 100,000 / 100,000 / 100,000 |
| normalized proposal bytes | 1 MiB | 8 MiB |
| speculative nodes/depth/unknown actions | 10,000 / 128 / 128 | 100,000 / 1,024 / 1,024 |
| open proof-debt obligations | 128 | 100,000 |
| translation attempts/work units | 100,000 / 100,000 | 1,000,000 / 1,000,000 |
| guarded candidates/guard dependencies | 10,000 / 1,024 | 100,000 / 100,000 |
| fallback depth | 16 | 64 |
| counterexample bytes | 1 MiB | 8 MiB |
| candidate-semantics-v2 events | 100,000 | 250,000 |
| generated speculative case size | 10,000 | 100,000 |
| equality spaces/revisions per workspace | 1,024 / 100,000 | 100,000 / 250,000 |
| equality nodes/edges per space | 10,000 / 100,000 | 100,000 / 1,000,000 |
| equality pending work/matches per node | 10,000 / 10,000 | 100,000 / 100,000 |
| equality expansion steps/saturation fuel | 1,024 / 10,000 | 100,000 / 100,000 |
| equality explanation depth/path edges | 1,024 / 1,024 | 100,000 / 100,000 |
| equality materialization steps/events | 1,024 / 100,000 | 100,000 / 250,000 |
| equality canonical/archive bytes | 64 MiB / 64 MiB | 128 MiB / 128 MiB |
| equality evaluation cases/elements | 1 / 50,000,000 | 1 / 500,000,000 |

The hard profile is used only while verifying/migrating/replaying persisted state. Lower interactive configuration therefore cannot arbitrarily make an archive unreplayable when it remains inside hard safety caps. A loaded workspace receives normal interactive defaults after successful publication.

## Check placement

- CLI retains at most request-limit bytes and discards the remainder of an oversized physical line.
- Protocol checks byte count and structural JSON depth before serde allocation.
- Core checks action count and projected dimensions/operations/values/holes/constraints/outputs before cloning a `Program` or allocating IDs.
- Region arguments, operations, operands and attribute bytes are checked before nested inference.
- Dynamic obligation count is checked immediately before obligation-ID allocation.
- Exact and semantic canonical encoders check byte size before hashing/publication.
- Store checks archive bytes during bounded read and checks revision/event/replayed-action counts before event application.
- Evaluator limits each input while flattening and projects every typed tensor plus total graph elements before allocating operation outputs.
- Candidate creation checks branch count before candidate ID allocation; candidate transactions check action count before graph clone.
- Rewrites check projected ImplIR/proof size before allocation; continuations stop at the deterministic match cap.
- Differential validation checks cases and accumulated tensor elements before generated arrays; EvidenceIR bytes are checked before publication.
- Proposal byte/action/fragment limits are checked before normalization, graph cloning and persistent ID allocation; projected debt/depth is checked before commit.
- Candidate canonical-v2 size, translation attempts, guard dependencies, fallback recursion and counterexample bytes are checked at their respective trust boundaries.
- Store checks candidate event/revision/evidence counts before candidate replay, which always uses hard safety caps.
- Equality creation/expansion stages its local allocator and complete revision before publication; match, node, edge, worklist, canonical-byte and event bounds are checked before commit.
- Explanation, debt discharge, evaluation and materialization independently bound traversal, proof path, tensor elements and replayed rewrite steps.

At exact limit the workload is accepted. Limit plus one returns `RESOURCE_LIMIT_EXCEEDED` with the resource kind, configured limit, attempted/actual value, context and a repair. Because graph/allocator/head state is staged, budget rejection is fully atomic.

These are robustness limits, not a multi-tenant security sandbox. Stage 2C still has no process isolation, authentication, locking or production server boundary.
