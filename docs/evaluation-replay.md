# Evaluation replay

Replay reads the recorded ordered decisions, resolves recorded menu choices from the exact observation frames, and invokes the production protocol engine in a fresh in-memory session. It compares every structured compiler response and independently verifies step order, observation hashes, episode hashes, evaluation hashes, budget accounting, and aggregates.

Replay never calls an agent/model, network, GPU, adapter discovery, or hardware benchmark. It does not trust a stored success flag: the archive loader validates hashes and the harness rebuilds compiler outcomes. A mismatch returns `EVALUATION_REPLAY_MISMATCH` or the more specific structural/hash diagnostic and publishes no replacement state.

For ranked episodes replay also reconstructs the exact choice set, feature schema, submitted fixed-point preferences, deterministic tie result and selection hash before dispatching the recorded selected action. It never reruns a ranker.

Stage 7A search replay is distinct: it reruns the retained scripted/learned ranker and every production branch outcome, objective vector, frontier order and stopping condition without training or external/device calls. See [search-replay.md](search-replay.md).

Stage 7B replay then verifies frozen production measurement records, cohort eligibility, integer aggregation, indifference ties and recommendation identity. It never reacquires hardware measurements; see [measured-search-replay.md](measured-search-replay.md).

Stage 7E replay composes the unchanged Stage 7A–7D replay paths, rechecks terminal materialization and campaign hashes, and reconstructs the Stage 7B recommendation. It accepts no executor and performs zero hardware, provider, or network calls; see [autotuning-campaign-replay.md](autotuning-campaign-replay.md).
