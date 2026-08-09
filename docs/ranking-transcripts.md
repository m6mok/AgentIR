# Ranking transcripts

`RankingTrace` anchors the observation, exact ordered choice set, feature schema, ranking policy, submitted integer scores or explicit selection, tie rule, final ordering, selected choice, and selection source. `SelectionOutcome` separately anchors the chosen visible action or hybrid escape to the exact production compiler outcome.

Replay rebuilds choice IDs and hashes, validates every score and tie, resolves membership, then executes only the recorded selected production action in a fresh compiler session. It performs no model, provider, network, device, or benchmark call. Ranking and selection hashes never substitute for compiler correctness hashes.
