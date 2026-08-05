# Stage 2C contract

Stage 2C completes the exact Stage 2 search foundation with a bounded, persistent equality space over whole typed ImplIR programs. It explores only compiler-owned production rewrites whose side conditions and certificates are already trusted. It does not admit agent rules, conditional equalities or observational claims.

The implemented contract includes:

- an equality space anchored to one explicit, fully proved, unconditional candidate revision;
- immutable `er*` revisions containing hash-consed `en*` whole-program nodes, trusted `ee*` proof edges and a deterministic worklist;
- canonical expansion and saturation with explicit fuel, hard resource caps and atomic rejection;
- root-to-member explanations reconstructed from compiler-verified edges;
- equality membership as a correctness path that may discharge the next matching speculative debt item;
- explicit materialization of a selected member through ordinary CandidateForest rewrite transactions;
- equality canonical/hash v1, candidate canonical/hash and event semantics v3, and archive/snapshot v6;
- JSONL create/query/expand/saturate/explain/evaluate/materialize/continuation operations.

An equality node is an entire verified `ImplProgram`, not an expression e-class. `impl_hash` is the semantic hash-cons key. Independent rewrite orders therefore merge when they reach the same reachable typed implementation, while distinct proof edges remain available for audit. Enumeration is ordered by stable rule ID and structural target locator; worklist order, edge deduplication and explanation tie-breaking are deterministic.

Only positive equality is represented. There is no disequality, refutation edge, untrusted certificate, rewrite DSL, congruence closure or extraction optimizer. `fixed_point` means that the finite reachable space under the current exact production registry and configured bounds has no pending work; it is not a global theorem about all equivalent programs.

Stage 2C adds exploration, not policy search. It has no ranking, cost model, beam/population search, approximate refinement, SMT, MemoryIR, ScheduleIR, target lowering, GPU, LLVM or MLIR integration. Evaluation is a deterministic semantic oracle and never establishes equality. Stage 3 begins with MemoryIR.
