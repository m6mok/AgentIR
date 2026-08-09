# Ranking features

Feature schema v1 is ordered, visible, and hashed with `agentir.evaluation.feature_schema.v1\0`. It defines action category, compiler layer, structural target, operand/result arity, visible type class, tensor/static/symbolic extent counts, obligation delta, proof-frontier effect, action bytes, continuation depth, fallback/escape flags, target capabilities, and an already-visible analytical resource estimate.

Normalization is deterministic and declared per feature. Hidden compiler state, future outcomes, task success, reference solutions, hidden preferred actions, unavailable benchmarks, and another device's measurements are forbidden. Feature ordering is semantic to the schema and changing it changes the hash.
