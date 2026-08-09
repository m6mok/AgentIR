# Stage 7C readiness

The narrow acquisition contract is ready when the full repository gate, Stage 7C tests, two-run comparator and unchanged Stage 7A/7B baselines pass.

- ownership is confined to `agentir-policy-eval`;
- hardware work requires explicit start/advance and server-owned metadata;
- canonical round robin, slot-boundary checkpointing and atomic in-memory publication are deterministic;
- replay and archive verification have zero device calls;
- complete results hand off through the existing Stage 7B cohort eligibility path;
- evaluation v5 migrates purely to v6; workspace v9 and measurement record v1 are unchanged.

This is not full Stage 7 completion. Real-device evidence is optional and machine-local; synthetic study success makes no performance, significance, portability, correctness or global-optimality claim.
