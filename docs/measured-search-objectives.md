# Measured search objectives

`MeasuredObjectiveDescriptor` v1 is separate from immutable `SearchObjectiveDescriptor` v1 and hashes under `agentir.evaluation.measured_objective.v1\0`. It anchors the cohort, metric, aggregation, minimize direction, records-per-artifact requirement, ppm indifference band, terminal-only eligibility, and exact structural fallback objective hash.

V1 permits `median_ns` and `p95_ns`. Multiple record summaries use the lower median after integer sorting; an even count selects the lower central member. All comparison arithmetic is checked integer arithmetic. Values are indifferent when `abs(a-b) * 1_000_000 <= min(a,b) * ppm`; overflow is a structured rejection. Intermediate Stage 7A nodes retain structural ordering and receive no latency estimate.
