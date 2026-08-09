# External ranker protocol

The bounded JSONL frontend adds `evaluation.ranking.policy.list`, `evaluation.ranking.policy.query`, `evaluation.choice_set.query`, `evaluation.episode.rank`, `evaluation.ranking.trace.query`, `evaluation.ranking.aggregate`, and `evaluation.ranking.compare`.

`evaluation.episode.rank` echoes run/episode/step, observation, choice-set, feature-schema and policy hashes, plus either exact fixed-point scores for every choice or one explicit visible choice. Optional usage and correlation IDs remain provenance. The request cannot supply legality, success, proof, compiler outcome, rejection class, metrics, hidden features, or future measurement. One input line produces one structured response.
