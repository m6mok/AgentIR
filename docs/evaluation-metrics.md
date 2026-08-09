# Evaluation metrics

Primary aggregates expose count, success rate, median/p95 decisions, rejections, repair cycles and context bytes, accepted actions per decision/token, budget exhaustion rate, and semantic correctness rate. No hidden failed episode and no unversioned weighted overall score is allowed.

Observation and decision bytes are deterministic UTF-8 measurements. The built-in optional accounting method `utf8_bytes_div4_v1` supplies a versioned deterministic token proxy. Provider-reported and agent-self-reported token counts retain distinct untrusted provenance. Missing counts are `null`/unknown, never zero.

Performance observations are optional confidence data anchored to artifact, measurement, and device fingerprint hashes. Cross-device values are not pooled. Performance never advances a proof and is not part of task success unless an explicit future corpus criterion says so.
