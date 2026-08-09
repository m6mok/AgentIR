# Stage 7B scope

Stage 7B is reproducible measurement-aware offline selection over terminal artifacts already reached by the frozen Stage 7A `deterministic_beam_v1` search. It lives only in `agentir-policy-eval`. Search expansion, structural frontier order, node/edge/checkpoint/trace/result bytes, compiler verification, and atomic production transactions are unchanged.

The layer resolves compiler-assigned measurement IDs or hashes from a verified production workspace, freezes an exact same-device/build/runtime/config/input cohort, and applies integer `median_ns` or `p95_ns` objectives only after search stops. It never calls `benchmark.start`, opens a device, estimates latency for intermediate nodes, publishes a live workspace, or advances correctness.

V1 supports menu-only scripted Stage 6B and archived learned Stage 6C rankers, one-record or lower-median-of-record-summary aggregation, checked ppm indifference bands, deterministic artifact-hash ties, checkpoint/resume/cancel, exact offline replay, and evaluation archive v5. Missing measurements are typed unavailable values. Cross-device pooling, live autotuning, energy, prediction, interpolation, free/hybrid generation, stochastic/population/concurrent search, and global-optimality claims are deferred.
