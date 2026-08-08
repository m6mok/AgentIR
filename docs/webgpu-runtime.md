# Optional WebGPU runtime

`agentir-runtime-wgpu` discovers adapters, hashes reported device provenance separately, checks it against the immutable WebGPU TargetManifest, builds storage/uniform bindings directly from the artifact ABI, dispatches exactly the selected plan and reads back named f32 outputs.

The default test suite requires no physical GPU. Offline compilation and WGSL validation always run. Device commands return structured `DEVICE_UNAVAILABLE` or capability diagnostics when no compatible adapter exists. Real-device differential tests are opt-in with `AGENTIR_RUN_GPU_TESTS=1`.

Runtime state has no correctness authority and cannot mutate BackendIR, ScheduleIR or artifact bytes.
