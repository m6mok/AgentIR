# Stage 7D readiness

Offline readiness is established by `tests/stage7d.rs` and two deterministic
`stage7d_study` runs compared by `stage7d_compare`. The study covers normal
completion, three crash boundaries, zero/one/multiple reconciliation, explicit
retry with a new attempt ID, explicit abandonment, Stage 7C result equivalence,
Stage 7B cohort/recommendation handoff, archive v7, and zero-device replay.

Run:

```bash
cargo test -p agentir-policy-eval --test stage7d
cargo run --release -p agentir-policy-eval --example stage7d_study -- --output target/stage7d-study/run-1
cargo run --release -p agentir-policy-eval --example stage7d_study -- --output target/stage7d-study/run-2
cargo run --release -p agentir-policy-eval --example stage7d_compare -- target/stage7d-study/run-1 target/stage7d-study/run-2
```

`timing-observations.json` is operational and excluded from comparison. The
synthetic executor is marked
`synthetic_test_data_not_performance_evidence`. Optional machine-specific
results belong only under `target/stage7d-study/device/`; unavailable production
artifacts or adapters produce a structured skip, never fake measurements.

The readiness verdict is intentionally narrow: Stage 7D v1 is ready as a
single-writer durable recovery and reconciliation contract. It is not full
Stage 7, an exactly-once hardware system, a correctness proof, or performance
evidence.
