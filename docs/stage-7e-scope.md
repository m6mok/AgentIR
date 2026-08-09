# Stage 7E scope

Stage 7E is an evaluation-only orchestration layer in `agentir-policy-eval`. It joins the existing Stage 7A search, Stage 7C acquisition, Stage 7D durable recovery, and Stage 7B measured recommendation contracts into one restartable campaign. It adds no search or ranking algorithm and has no compiler-correctness authority.

The lifecycle is `Planned → Searching → SearchComplete → AcquisitionPrepared → Acquiring/RecoveryRequired → AcquisitionComplete → CohortReady → RecommendationReady → Complete`. `NoEligibleTerminal`, `Cancelled`, `Blocked`, and `Failed` are explicit stopping states. Every state-changing request carries the exact current campaign-session hash and publishes staged state only after validation.

Only `evaluation.autotuning_campaign.execute_prepared` may receive the server-owned executor or perform benchmark/device work. Search, materialization, reconciliation, cohort creation, recommendation, status, checkpoint, resume, result, replay, and archive verification are zero-device operations.

V1 is single-workspace and single-writer. It makes no exactly-once physical-execution, globally-optimal, proven-fastest, statistical-significance, portability, correctness, or live-publication claim. Concurrency, remote workers, device pools, prediction, training, interpolation, energy objectives, and new search/ranking policies remain deferred.
