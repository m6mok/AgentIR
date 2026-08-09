# Ranking fairness

Ranked runs are comparable only when the Stage 6A corpus/build/seed/task/budget/runtime/success anchors match and their exact ordered `choice_set_hash` sequence, `feature_schema_hash`, and permitted escape surface also match. Device metrics additionally require identical fingerprints.

Different hidden features, choice frames, or escape rights return `EVALUATION_RANKING_COMPARISON_INVALID`. Rejected, failed, and budget-exhausted episodes remain visible.
