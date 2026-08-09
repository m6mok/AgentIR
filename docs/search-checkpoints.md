# Search checkpoints

`SearchCheckpoint` v1 stores prefix/version/digest, root/objective/plan/ranker/model anchors, exact nodes/edges/frontier, terminal set, duplicate index, deterministic next-work cursor, lifecycle status, result/trace state, and work accounting. Its independent domain is `agentir.evaluation.search_checkpoint.v1\0`.

Resume checks byte limits, prefix/version/digest, all anchors, cursor, node/edge hashes, parent graph, cycle absence, and frontier membership/order before branch execution. Complete and cancelled checkpoints resume idempotently. A corrupt, stale or future checkpoint performs no production action.

One semantic work unit expands one parent. Cooperative cancellation is observed only between units. It retains all published semantic nodes and produces `cancelled`, never `complete`. Advance-call partitioning and manual checkpoint/resume do not enter semantic trace/result identity; single-shot and every-unit resume produce byte-identical semantic traces and results.
