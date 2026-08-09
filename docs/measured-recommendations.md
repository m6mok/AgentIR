# Measured recommendations

`MeasuredRecommendation` v1 hashes under `agentir.evaluation.measured_recommendation.v1\0`. It binds the Stage 7A result, measured objective, cohort, selected terminal/artifact, exact measurement hashes, aggregate integer value, tie classification, ordered eligible alternatives, and typed ineligible alternatives.

Selection occurs only among measured terminal artifacts. Numeric order is ascending; alternatives inside the descriptor's indifference band are measurement-equivalent and resolve by artifact hash then search-node ID. The record says “selected under descriptor X from cohort Y; not proven fastest.” If no eligible measured terminal exists, it returns stable `no_comparable_measured_terminal` and selects nothing.
