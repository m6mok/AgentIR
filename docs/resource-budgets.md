# Resource budgets

Stage 6A owns additional operational hard limits for corpora/tasks/task bytes, runs/episodes/steps, observations/menu choices/action/transcript bytes, decisions/rejections/repair cycles, deterministic and reported token counts, cumulative context, active runs, aggregates/comparisons, evaluation archive bytes, replay work units, and generated property cases. Task budgets are immutable corpus data for fair comparison; harness hard caps are operational. Neither class enters any compiler correctness hash, while fixed task budgets intentionally enter `corpus_hash`.

Stage 6B additionally bounds choices per frame, bytes per choice/set, feature definitions/features/bytes, policies, score entries and magnitude, tie groups, ranking traces/transcript bytes, decisions, selections, migration work, replay work, and generated multi-choice cases. Limit failures occur before trace, selection, or archive publication and enter no compiler hash.

Memory budgets cover plans/revisions/events, buffers/operations/accesses, alias facts/domains, lifetime uses, obligations, reuse attempts, guard dependencies and fallback depth, layout entries, per-buffer/total abstract bytes, canonical/archive bytes, evaluation elements, trace events/bytes, and generated cases. Projected hard-limit failures happen against a staged store before publication; resource policy never enters `memory_hash`.

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
| memory plans/revisions/events | 1,024 / 100,000 / 100,000 | 100,000 / 250,000 / 250,000 |
| memory buffers/operations/accesses | 100,000 / 100,000 / 500,000 | 1,000,000 / 1,000,000 / 5,000,000 |
| memory alias domains/facts | 100,000 / 500,000 | 1,000,000 / 5,000,000 |
| memory lifetime points/uses | 500,000 / 500,000 | 5,000,000 / 5,000,000 |
| memory obligations/reuse attempts | 100,000 / 1,024 | 1,000,000 / 100,000 |
| memory guard dependencies/depth | 1,024 / 16 | 100,000 / 64 |
| memory fallback depth | 16 | 64 |
| memory layout rank/stride entries | 1,024 | 4,096 |
| memory allocation bytes per buffer/total | 8 GiB / 64 GiB | 64 GiB / 512 GiB |
| memory canonical/archive bytes | 64 MiB / 64 MiB | 128 MiB / 128 MiB |
| memory evaluation elements | 50,000,000 | 500,000,000 |
| memory trace events/bytes | 1,000,000 / 64 MiB | 10,000,000 / 128 MiB |
| generated MemoryIR case size | 10,000 | 100,000 |

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
- Memory creation and transactions stage the memory-local allocator, plan and complete revision before publication; buffer, operation, access, alias, lifetime, obligation, allocation, canonical and event bounds are rechecked structurally.
- Compiler-owned guarded reuse independently bounds guard dependencies/depth and the exact fallback depth; memory evaluation bounds semantic elements plus deterministic trace events/bytes.
- Archive v7 replay applies hard memory plan/revision/event/archive bounds before publishing the reconstructed workspace.

At exact limit the workload is accepted. Limit plus one returns `RESOURCE_LIMIT_EXCEEDED` with the resource kind, configured limit, attempted/actual value, context and a repair. Because graph/allocator/head state is staged, budget rejection is fully atomic.

These are robustness limits, not a multi-tenant security sandbox. Stage 3 still has no process isolation, authentication, locking or production server boundary.
# Stage 4 budgets

Central limits cover target manifests/revisions/events/capabilities/canonical bytes; schedule plans/revisions/events/nodes/axes/transforms/fusion/dependencies/legality facts; tile rank, remainders, bindings, vector width, unroll factor and launch dimensions; simulator coordinates/work; obligations/evidence/canonical/archive bytes; schedule trace events/bytes; and generated schedule cases. Checks use checked or saturating arithmetic and stage mutations so hard-limit rejection consumes no persistent ID. Limits never participate in `target_hash`, `schedule_hash`, or archive semantic state.

# Stage 5 budgets

Central limits cover backend plans/revisions/events/kernels/values/expressions/statements/source nodes/bindings/parameters/dispatches/guards/work units/obligations/evidence/canonical bytes; artifact packages/modules/entry points/WGSL/manifest/events/validation work; execution buffers/elements/bytes/traces/tasks; benchmark warmups/iterations/records/bytes/wall time; generated cases; and backend/archive bytes. Checks precede persistent ID, shader, buffer, device-task, and publication work. Policy remains excluded from all correctness and reproducibility hashes.
