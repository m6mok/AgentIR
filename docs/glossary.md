# Glossary

- **ActionIR** — typed algebra of graph edits submitted by an agent.
- **Archive** — checksummed versioned encoding of a workspace snapshot and replay log.
- **Canonical state** — deterministic serialized `Program` used for hashing and replay.
- **Compiler core** — transport-independent verifier and workspace state machine.
- **ContinuationFrame** — parameteric description of legal next choices for a focused task.
- **Hole** — missing pure value with a persistent ID and required type/shape.
- **ImplIR** — future layer describing an algorithmic implementation of frozen SpecIR.
- **MemoryIR** — future layer describing physical storage and mutation.
- **Obligation** — explicit proposition that is open, proved, refuted or unsupported.
- **Persistent ID** — compiler-assigned identity such as `v4`, `h1` or `r2`.
- **Region** — closed pure block with typed arguments, explicit captures and a yield.
- **Revision** — immutable workspace snapshot with parent links and content hash.
- **ScheduleIR** — future mapping of work to target hardware.
- **SpecIR** — functional graph describing what must be computed.
- **Temporary binding** — `$name` usable only within one transaction.
- **Workspace** — revision DAG plus compiler-owned ID state; it is live in memory and may be persisted to a verified local archive.
