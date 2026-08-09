# Multi-choice continuations

`EvaluationChoiceSet` is the exact ordered view of compiler-generated production actions for one observation. Each `EvaluationChoice` has a stable `ec…` identity derived from layer, category, typed action, visible bases/preconditions, and compiler order. Reordering changes `choice_set_hash`; identical compiler state and ordering reproduce IDs and bytes.

The evaluation adapter expands only bounded production continuation descriptors. Candidate matches become exact `candidate.apply` actions; MemoryIR reuse descriptors become fresh/static attempts; ScheduleIR axes/factors/fusion/vector/unroll domains become individual checked `schedule.apply` actions; compatible hole values become `fill_hole` transactions. No choice carries a hidden score or future outcome.

Ranking is read-only. Only `SelectionDecision` dispatches one resolved action through `agentir-protocol::Engine`. Missing membership, stale hashes, duplicate IDs, or limit overflow reject before compiler mutation.
