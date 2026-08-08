# Device fingerprints

`DeviceFingerprint` is runtime provenance, separate from immutable `TargetManifest`. It records the WebGPU backend/API, adapter name, optional vendor/device IDs, driver/backend information, reported limits, runtime version, and compiler version.

The compiler-owned `webgpu_wgsl_v1` manifest is a minimum capability contract. Discovery checks an adapter against that contract but never mutates the manifest or `target_hash`. `device_fingerprint_hash` uses its own `agentir.device.fingerprint.v1\0` domain and is excluded from SpecIR, ImplIR, MemoryIR, ScheduleIR, BackendIR, artifact, and archive semantic identities.

A fingerprint is neither a correctness certificate nor a portable device identity. Device absence returns `DEVICE_UNAVAILABLE` without consuming persistent IDs or mutating the workspace.
