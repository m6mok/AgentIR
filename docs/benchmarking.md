# Benchmark baseline schema v2

Schema v2 is retained for Stage 3. The baseline reports median/p95 MemoryIR fresh bufferization and buffer/lifetime analysis at 10/100/1,000 operations or buffers, alias queries at 1/10/100 domains, verification, canonicalization, hash/query, reuse/rejection/guard construction, true/false evaluation, fork/seal, v6→v7 migration and v7 replay of fresh/reused/guarded/equality-materialized histories. Canonical byte sizes for fresh MemoryIR, buffers, accesses, alias/lifetime facts, certificates, guarded fallback, exact memory state and archive variants remain separate from timing and are never committed as machine-specific output.

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

Stage 2C retains schema v2 and adds equality creation for 10/100/1,000-operation roots; one-step and 1/10/100-step expansion; hash-cons merge and edge-dedup paths; fixed-point and resumed saturation; repeated equality-hash queries; 1/10/100-edge explanations; equality debt discharge and explicit materialization; v5 → v6 migration; and v6 replay for root/expanded/saturated/discharged/materialized histories. Byte records cover equality roots/nodes/edges/worklists/explanations, candidate v3 equality proof state and the corresponding archive v6 states.

## Stage 1.1 reference

The pre-change single-shot run at commit `5a838d8540f57d7e171c1e333cd008957c1c60cd` on macOS/aarch64 recorded the following nanoseconds: transaction apply `{1: 215416, 10: 53708, 100: 395333}`, shape query 100k `1715750`, exact serialization 10k `16144584`, semantic canonicalization `{10: 27917, 100: 176500, 1000: 1984458}`, spec-hash query 1k `140525208`, continuation 10k `26016667`, SAXPY `{4: 75500, 1024: 493334, 65536: 31123458}`, v1→v2 migration 100 `7238292`, and v2 replay 100 `6368333`.

Those values were single-shot, some include setup effects, and they are not directly comparable to median schema-v2 records. They are historical orientation, not a performance contract.

The SAXPY, candidate, equality and memory evaluators are deterministic CPU semantic/physical oracles. Their numbers say nothing about GPU code generation, device throughput, kernels, transfers or launch overhead; Stage 3 contains no GPU backend.
# Stage 4 workloads

Benchmark schema v2 remains wire-compatible. Stage 4 coverage measures median/p95 serial construction at 10/100/1,000 operations and domain construction at 10/100/1,000 axes, dependency analysis, split/tile/remainder, legal and rejected fusion, binding, vector/unroll proof paths, resource simulation, verification/canonicalization/hash query, fork/seal, serial/tiled/remainder/guarded evaluation, v7 → v8 migration, and v8 replay variants. Canonical sizes are reported separately for TargetManifest, schedule state, axes/domains, dependencies, fusion, bindings, estimates, and certificates. Machine-specific output is never committed.

# Stage 5 workloads

The dependency-light baseline remains GPU-independent and adds BackendIR lowering/verification, deterministic WGSL emission, offline Naga validation, artifact hashing, v8→v9 migration and v9 replay. Device timing is recorded separately through the bounded benchmark protocol described in [hardware-benchmarking.md](hardware-benchmarking.md); those machine-specific measurements are confidence evidence and are not committed as baseline expectations.

# Stage 6A workloads

Benchmark schema v2 remains unchanged. `agentir-policy-eval --example evaluation_baseline` reports deterministic scripted free/menu/hybrid SAXPY completion and replay. The evaluation workload contract covers corpus canonicalization at 10/100/1,000 tasks; observation/menu construction; free/hybrid validation; step apply; rejection and repair accounting; episode hashing/replay at 10/100/1,000 steps; aggregates/comparisons; archive save/load; and mutation rejection. Canonical sizes for task, observation, menu, decision, outcome, transcript, episode, aggregate, and archive are separate from timing. Machine-specific output is never committed.

# Stage 6B workloads

Schema v2 remains unchanged. The evaluation baseline covers choice enumeration/canonicalization/hash at 10/100/1,000 choices, visible feature extraction/schema hashing, scripted ranking, external score validation, tie resolution, explicit selection dispatch, rejection classification, trace replay/aggregation/fairness, v1→v2 migration, v2 save/load/replay, and mutation rejection. Canonical sizes are reported separately and machine-specific output is never committed.

`stage6b_study` extends the local, uncommitted dataset with raw samples and min/median/p90/p95/p99/max/mean/standard deviation; exact input digests; accepted/policy/compiler/budget rejection classes; fixed-seed production episodes; archive mutations; and a generated analysis. `stage6b_compare` treats `semantic.json` differences as failures while classifying compatible timing differences as expected machine noise. Representative combinations are bounded and every skip carries a stable reason code.

# Stage 6C workloads

Schema v2 remains unchanged. `stage6c_study` measures dataset extraction/leakage validation/splitting, visible feature encoding, integer training, model serialization/hash, fixed-point inference, score/tie validation, production dispatch, replay, current evaluation archive verification, and failure fast paths. Raw min/median/p90/p95/p99/max/mean/stddev samples stay under `target/stage6c-study/`; `stage6c_compare` requires byte-identical semantic files and archive bytes while excluding timing.

# Stage 7A workloads

`stage7a_study` measures root initialization, bounded search, deterministic continuation/ranking/branch counters, checkpoint encode/verify, full replay and evaluation archive v4 validation. It runs scripted and learned beam widths 1/2/4 over a documented deterministic task subset. `stage7a_compare` requires byte-identical semantic, result, checkpoint, mutation-classification and archive files; timing median/p95/p99 fields remain machine observations only.

# Stage 7B workloads

`stage7b_study` uses explicitly synthetic, production-format-verified fixture records to exercise three proved/offline-valid artifacts of one `spec_hash`, scripted/learned rankers, beam widths 1/2/4, median/p95 objectives, normal/indifference cohorts, structured rejection, replay, and archive v5. `stage7b_compare` requires byte-identical semantic/recommendation/checkpoint/mutation/archive files and excludes timing observations. Synthetic values are contract fixtures, never hardware-performance evidence. Optional real-device acquisition remains a separate `benchmark.start` operation and writes only under `target/stage7b-study/device/`.
