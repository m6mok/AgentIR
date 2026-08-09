# Stage 6A scope

Stage 6A is a reproducible, non-correctness evaluation layer above the completed Stage 1–5 compiler. The separate `agentir-policy-eval` crate compares `free`, `menu`, and `hybrid` interaction policies using immutable tasks, fixed budgets, exact observations, recorded decisions, production compiler outcomes, compiler-owned success predicates, deterministic replay, and raw statistical aggregates.

Evaluation never changes SpecIR, ImplIR, MemoryIR, ScheduleIR, BackendIR, artifact, measurement, or workspace archive v1–v9 contracts. Every submitted production request is decoded and executed by `agentir-protocol::Engine`; policy choice, testing, tokens, latency, and device measurements cannot create correctness evidence.

The default corpus is offline and contains twenty ordered categories from SpecIR construction through equality-to-artifact, archive replay, and end-to-end SAXPY. Five scripted policies provide deterministic CI coverage. External agents use bounded JSONL and no provider SDK is present in core or the harness.

Stage 6A has no autotuning, learned policy, prompt optimization, cost-model ranking, population/beam search, schedule generation, automatic extraction, or best-artifact selection. Those remain Stage 6B or later.
