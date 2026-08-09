# Search objectives

`SearchObjectiveDescriptor` v1 anchors the exact corpus, task definition and immutable initial state. Its independent `search_objective_hash` uses `agentir.evaluation.search_objective.v1\0`; an operational display label is explicitly excluded.

The comparison contract is `ordered_lexicographic_checked_i64_v1`. Components remain individually interpretable and ordered. V1 permits compiler terminal success, evaluation task-criterion success, accepted progress count, open obligation count, rejection count, repair count, deterministic trajectory work, accumulated fixed-point ranking score, and trajectory length. Success/progress/score maximize; obligations/rejections/repairs/work/length minimize. Empty, duplicated, unknown, direction-incompatible, overflowing, hardware, timing, provider, future-outcome, reference-solution, label, and split-derived objectives reject.

Task success, ranking score and objective values are evaluation data. None advances a compiler proof frontier. Incomplete searches publish only a bounded-frontier recommendation.
