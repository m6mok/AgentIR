# Evaluation corpus

`EvaluationCorpus` is an ordered immutable list of `EvaluationTask` values. Every task fixes its stable ID/version/category/size, objective, deterministic initial production requests, allowed interaction modes, action/rejection/context/token/time budgets, runtime inputs, required final state, optional reference output/target, and task-independent metadata. The exact ordered definition is covered by `corpus_hash`; reordering tasks changes the hash.

The built-in `stage6a-v1` corpus covers SpecIR construction, hole repair, constraints, exact and speculative candidates, equality, fresh/static/guarded memory, serial/tiled/fused/vector schedules, illegal-schedule repair, backend lowering and unsupported lowering, artifact emission, equality-to-artifact, archive replay, and SAXPY. Small, medium, and large labels are stable corpus data. Default tasks require neither network nor a physical GPU.

Corpus budgets participate in corpus identity because fairness requires the same experimental allowance. Global hard safety limits are operational and do not enter compiler identities.
