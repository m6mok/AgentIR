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

At exact limit the workload is accepted. Limit plus one returns `RESOURCE_LIMIT_EXCEEDED` with the resource kind, configured limit, attempted/actual value, context and a repair. Because graph/allocator/head state is staged, budget rejection is fully atomic.

These are robustness limits, not a multi-tenant security sandbox. Stage 1.2 still has no process isolation, authentication, locking or production server boundary.
