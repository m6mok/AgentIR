# Documentation index

Start with the root [README](../README.md), then choose the document matching the task:

- [Getting started](getting-started.md) — build and run SAXPY in a few minutes.
- [Architecture](architecture.md) — data flow, crate boundaries, transactions and revisions.
- [Stage 1 scope](stage-1-scope.md) — implemented contract, invariants and explicit non-goals.
- [Stage 2A scope](stage-2a-scope.md) — exact candidate foundation and explicit boundary.
- [ImplIR](implir.md) — separate implementation graph, identity lowering, verifier and hash.
- [CandidateForest](candidate-forest.md) — candidate branches, revisions, transactions and continuations.
- [Equivalence and EvidenceIR](equivalence-and-evidence.md) — trusted proof composition and confidence evidence.
- [JSONL protocol](protocol.md) — commands, references, regions, errors and response envelopes.
- [Persistence and replay](persistence.md) — archive format, save/load lifecycle and integrity model.
- [Semantic canonicalization](semantic-canonicalization.md) — `spec_hash`, alpha-normalization and graph identity limits.
- [Constraint solving](constraint-solving.md) — compact facts, soundness and obligation discharge.
- [Resource budgets](resource-budgets.md) — interactive defaults, hard caps and check placement.
- [Benchmarking](benchmarking.md) — statistical schema v2, SpecIR/candidate workloads and historical references.
- [Development](development.md) — repository layout, checks, tests and baseline harness.
- [Glossary](glossary.md) — short definitions of AgentIR terms.
- [Roadmap](roadmap.md) — sequencing from the prototype to a GPU backend.
- [Reference sources](reference/) — the complete AgentIR 0.1 draft and Stage 1 implementation brief used to bootstrap this repository.

Architectural trade-offs and known prototype limits are tracked in [DECISIONS.md](../DECISIONS.md).
