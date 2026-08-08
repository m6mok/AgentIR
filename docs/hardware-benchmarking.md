# Hardware benchmarking

`benchmark.start` accepts a retained `artifact_hash`, adapter selector, runtime inputs, and bounded warm-up/measured iteration configuration. The runtime executes only the compiler-emitted package and publishes min, median, p95, max, input distribution, tensor dimensions, guard outcome counts, validation status, target/build hashes, device fingerprint, and runtime version.

Completed records use the independent `agentir.measurement.hardware.v1\0` hash domain. They are confidence evidence only: they cannot modify ScheduleIR, choose an artifact, rank plans, change correctness hashes, or close proof obligations. Archive replay validates record provenance, event order, allocator state, and `measurement_hash` without repeating hardware work.

The JSONL lifecycle exposes `benchmark.start`, `benchmark.status`, `benchmark.cancel`, and `benchmark.query`. The current bounded implementation completes synchronously; task records preserve the stable lifecycle surface. Device-dependent tests remain opt-in through `AGENTIR_RUN_GPU_TESTS=1`.
