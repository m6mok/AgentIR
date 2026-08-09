# Evaluation replay

Replay reads the recorded ordered decisions, resolves recorded menu choices from the exact observation frames, and invokes the production protocol engine in a fresh in-memory session. It compares every structured compiler response and independently verifies step order, observation hashes, episode hashes, evaluation hashes, budget accounting, and aggregates.

Replay never calls an agent/model, network, GPU, adapter discovery, or hardware benchmark. It does not trust a stored success flag: the archive loader validates hashes and the harness rebuilds compiler outcomes. A mismatch returns `EVALUATION_REPLAY_MISMATCH` or the more specific structural/hash diagnostic and publishes no replacement state.

For ranked episodes replay also reconstructs the exact choice set, feature schema, submitted fixed-point preferences, deterministic tie result and selection hash before dispatching the recorded selected action. It never reruns a ranker.
