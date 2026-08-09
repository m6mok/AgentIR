# Ranking transcripts

`RankingTrace` anchors the observation, exact ordered choice set, feature schema, ranking policy, submitted integer scores or explicit selection, tie rule, final ordering, selected choice, and selection source. `SelectionOutcome` separately anchors the chosen visible action or hybrid escape to the exact production compiler outcome.

Replay rebuilds choice IDs and hashes, validates every score and tie, resolves membership, then executes only the recorded selected production action in a fresh compiler session. It performs no model, provider, network, device, or benchmark call. Ranking and selection hashes never substitute for compiler correctness hashes.

`choice_id` uses the independent `agentir.evaluation.choice_id.v1\0` domain over the compiler layer, category, semantic production request, and hard preconditions. Compiler order remains a choice-set property and is excluded from identity. Transport-only `request_id`, external correlation IDs, timestamps, latency, hostname, and provider-session fields are recursively excluded; mutating the production action, bases, capabilities, or hard conditions changes the identity.
