# Hardware benchmarking

`benchmark.start` accepts a retained `artifact_hash`, adapter selector, runtime inputs, and bounded warm-up/measured iteration configuration. The runtime executes only the compiler-emitted package and publishes min, median, p95, max, input distribution, tensor dimensions, guard outcome counts, validation status, target/build hashes, device fingerprint, and runtime version.

Completed records use the independent `agentir.measurement.hardware.v1\0` hash domain. They are confidence evidence only: they cannot modify ScheduleIR, choose an artifact, rank plans, change correctness hashes, or close proof obligations. Archive replay validates record provenance, event order, allocator state, and `measurement_hash` without repeating hardware work.

The JSONL lifecycle exposes `benchmark.start`, `benchmark.status`, `benchmark.cancel`, and `benchmark.query`. The current bounded implementation completes synchronously; task records preserve the stable lifecycle surface. Device-dependent tests remain opt-in through `AGENTIR_RUN_GPU_TESTS=1`.

Stage 7B consumes these completed records only after publication and verification. Measured search/replay cannot invoke this lifecycle; optional same-device acquisition remains an explicit operation completed before cohort creation.

Stage 7C orchestrates repeated explicit acquisition without changing `HardwareMeasurementRecord` v1. Start verifies artifacts and performs server-owned adapter/runtime preflight; advance runs only full canonical slots. Device fingerprint, build/runtime, validation and timing remain server owned. A failed/unavailable slot publishes no fake record. Checkpoint resume revalidates anchors before hardware work, while result replay and archive verification perform zero device calls.

Stage 7D does not change the record or benchmark contract. It persists the exact pending slot and a server-owned publication snapshot before hardware authorization. A crash after benchmark is indeterminate rather than a numeric sentinel or automatic retry. Reconciliation rehashes post-boundary production records and performs zero device calls. Physical exactly-once execution is explicitly not claimed.

Stage 7E does not change `HardwareMeasurementRecord` v1. Campaign search, prepare, status, checkpoint, resume, reconcile, cohort, recommendation, result, replay, and archive verification cannot open a device. Only explicit `evaluation.autotuning_campaign.execute_prepared` invokes the existing Stage 7D prepare/execute path.
