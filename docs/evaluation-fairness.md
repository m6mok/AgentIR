# Evaluation fairness

Runs are comparable only when corpus hash, compiler build, exact seed set, task definitions and versions, budgets, initial state/archive hashes, runtime inputs, and success criteria agree. Device metrics additionally require the same `device_fingerprint_hash`. The comparison API rejects incompatible runs instead of silently normalizing them.

Every policy receives the same task order and budget. Surfaces intentionally differ, but hidden compiler state is never added to an observation. Failed and budget-exhausted episodes remain in the denominator. Reports retain raw distributions rather than collapsing them into one opaque score.
