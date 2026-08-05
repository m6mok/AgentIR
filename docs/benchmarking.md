# Benchmark baseline schema v2

Run the dependency-light harness in release mode:

```bash
cargo run --release -p agentir-protocol --example baseline
```

The output is one JSON object with `benchmark_schema_version = 2`, environment/build metadata, named timing records and separate canonical byte sizes. Every timing has warm-up count, measured count, min, median, p95, max, nanosecond unit and structured workload size.

Median is the typical local sample after warm-up. P95 is the smallest sorted sample at or above the 95th percentile and exposes tail variability; min/max help identify timer granularity and outliers. Compare measurements only for the same workload, build mode, architecture, OS and broadly comparable machine state. A median regression above 2× is an investigation signal, never a CI threshold.

## Workloads

The harness retains Stage 1.1 workloads:

- transaction apply for 1/10/100 operations;
- 100,000 shape queries;
- 10,000 exact-state serializations;
- semantic canonicalization for 10/100/1,000 reachable operations;
- 1,000 repeated `spec_hash` queries;
- 10,000 continuation frames;
- SAXPY reference evaluation for 4/1,024/65,536 elements;
- v1 → v2 migration and v2 load/replay.

Stage 1.2 adds constraint insertion/query at 10/100/1,000 facts, transitive discharge, contradiction detection, resource-rejection fast path, v2 → v3 migration, v3 replay and mixed-semantics replay. Canonical byte sizes are not timings and appear in a separate object.

Stage 2A keeps schema v2 and adds identity lowering/candidate creation/ImplIR canonicalization for 10/100/1,000 operations; candidate fork/apply/seal; known-rewrite and constant-fold match scans; continuation generation; repeated `impl_hash`/`candidate_hash` queries; equivalence-chain verification; fixed-seed differential validation; v3 → v4 migration; and v4 replay with and without candidate history. Byte sizes separately report SpecIR semantic, ImplIR semantic, exact candidate state and archive encodings.

Stage 2B also keeps schema v2. It measures proposal normalization/hash at 10/100/1,000 operations, speculative transaction apply, debt insertion at 1/10/100 steps, frontier and hash-v2 verification, known/identity/unsupported/guarded validation, true and false guarded evaluation, refutation publication, speculative continuation escape, v4 → v5 migration, and v5 replay for exact/speculative/guarded/refuted histories. Separate byte records cover proposal canonical form, candidate exact v1/v2, proof debt and archive v5.

## Stage 1.1 reference

The pre-change single-shot run at commit `5a838d8540f57d7e171c1e333cd008957c1c60cd` on macOS/aarch64 recorded the following nanoseconds: transaction apply `{1: 215416, 10: 53708, 100: 395333}`, shape query 100k `1715750`, exact serialization 10k `16144584`, semantic canonicalization `{10: 27917, 100: 176500, 1000: 1984458}`, spec-hash query 1k `140525208`, continuation 10k `26016667`, SAXPY `{4: 75500, 1024: 493334, 65536: 31123458}`, v1→v2 migration 100 `7238292`, and v2 replay 100 `6368333`.

Those values were single-shot, some include setup effects, and they are not directly comparable to median schema-v2 records. They are historical orientation, not a performance contract.

The SAXPY and candidate evaluators are deterministic CPU semantic oracles. Their numbers say nothing about GPU code generation, device throughput, kernels, transfers or launch overhead; Stage 2B contains no GPU backend.
