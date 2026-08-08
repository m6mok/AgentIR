# Hardware benchmark records

`benchmark.start` is bounded by central warm-up, iteration, wall-time, execution-size and record-size limits. The synchronous JSONL engine returns a compiler-owned completed task handle and publishes an immutable measurement record containing min, median, p95 and max time, the input distribution and dimensions, artifact/target/build hashes, device fingerprint, runtime version, guard outcomes and validation status.

Measurements use the separate `agentir.measurement.hardware.v1\0` hash domain. They are confidence evidence only: they do not select an artifact, change any compiler graph or advance the proof frontier. Archive replay verifies structure and provenance but never reruns hardware work.
