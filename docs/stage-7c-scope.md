# Stage 7C scope

Stage 7C is an evaluation-only, explicit, bounded acquisition orchestrator in `agentir-policy-eval`. It measures only retained proved/offline-valid terminal artifacts from one production workspace, target, compiler build, runtime, device fingerprint and benchmark configuration. It never advances correctness, changes Stage 7A/7B contracts, publishes a live artifact, or starts search automatically.

V1 supports server-owned preflight, canonical artifact-hash round robin, fixed records per artifact, start/advance/status/checkpoint/resume/cancel/result/replay, typed unavailable/failure outcomes, a separate Stage 7B cohort handoff, and evaluation archive v6. Hardware work occurs only in start preflight and explicit advance. Replay, archive load, recommendation and ordinary resume execute no benchmark.

Concurrency, remote workers, multi-device pooling, prediction/interpolation, energy objectives, raw-sample significance, new search algorithms, training during acquisition and global-optimality/performance-proof claims are deferred.
