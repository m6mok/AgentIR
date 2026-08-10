# Stage 8B scope: bounded CPU measurement

Stage 8B is a separate, local, single-process non-correctness layer over already published Stage 8A `cpu_scalar_v1` packages. `agentir-runtime-cpu` owns monotonic clock access, bounded warmups and measured iterations, execution through the unchanged Stage 8A interpreter, output-consistency checking, raw integer-nanosecond samples, and deterministic min/median/nearest-rank-p95/max aggregation. It performs no lowering.

## Trust and identity boundary

The client selects a workspace, CPU artifact ID, exact `cpu_artifact_hash`, ordinary inputs, and bounded v1 configuration. Samples, aggregates, host data, output anchors, measurement hashes, bytecode, ABI, guards, certificates, and success claims are never request fields. Only `cpu_measurement.acquire` executes bytecode or reads a clock; `list`, `query`, `check`, archive verification, migration, and replay are structural and zero-execution.

The independent versioned domains are `cpu_benchmark_config_hash`, `cpu_input_hash`, `cpu_host_fingerprint_hash`, `output_hash`, and `cpu_measurement_hash`. Complete measurement identity covers the exact artifact/build/runtime/config/input/host provenance, ordered samples, aggregates, and agreed outputs. Store-local `cpum*` IDs, resource limits, paths, request/workspace/process IDs, timestamps, and polling history are excluded. Resource policy still bounds acquisition and archive publication.

Synthetic clocks exist only through the explicit injectable runtime test API, carry `synthetic_test_fixture_v1`, and cannot share a `CpuMeasurementStore` with production-clock records.

## Persistence and authority

Workspace archive/snapshot v11 adds `CpuMeasurementStore`. V1–v10 remain immutable inputs; v10→v11 adds an empty store and invents no measurements, fingerprints, timings, or execution history. Native replay verifies record hashes, artifact anchors, event order, dependency cursors, and allocator state without executing bytecode or reading a clock.

A CPU measurement is an observation only. It cannot advance any proof frontier, change compiler/artifact hashes or bytecode, select or publish an artifact, rank candidates, or prove correctness, speedup, significance, portability, superiority, or global optimality. SIMD, threads, native code/JIT, reductions, remote workers, retries, energy records, recommendations, search, and autotuning are outside Stage 8B.
