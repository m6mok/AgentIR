# Measurement acquisition plans

`MeasurementAcquisitionPlan` v1 hashes under `agentir.evaluation.measurement_acquisition_plan.v1\0`. It anchors workspace/root, corpus/task, sorted exact artifact hashes, shared spec/target/build, requested config, positive records-per-artifact, validation/failure/order policy and checkpoint cadence.

V1 order is `round_robin_artifact_hash_v1`: sorted artifacts A/B/C with three records produce A,B,C,A,B,C,A,B,C. Request order is ignored; duplicates, missing/unproved/offline-invalid artifacts, mixed spec/target/build and invalid configurations reject before session publication. Timing, timestamps, limits, paths, request IDs, counters and transient errors are excluded from plan identity.

Default operational caps are 1,024 artifact references, 10,000 records per artifact, 1,000,000 total slots and 256 MiB per encoded checkpoint. Exact limits accept, limit plus one rejects before slot-vector allocation, and alternate limits that admit the same request do not change the plan hash.
