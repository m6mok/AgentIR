# WebGPU/WGSL backend

`agentir-backend-wgsl` is the trusted Stage 5 lowering and emission component. It accepts only the built-in `webgpu_wgsl_v1` target and a proved ScheduleIR/MemoryIR/ImplIR chain. Clients cannot supply BackendIR, WGSL, bindings, guards or certificates.

The v1 ABI uses bind group zero. Storage buffers are ordered by compiler buffer ID; read-only inputs use `var<storage, read>`, outputs and proved reuse use `read_write`. Scalar captures and a symbolic extent occupy a deterministic 16-byte-aligned uniform block. One-dimensional f32 buffers may use widths 1, 2 or 4; all accesses retain exact buffer offsets and compiler-owned tail bounds.

Emission is deterministic UTF-8 with LF line endings. Every module is parsed and validated with Naga before an artifact can be published. Parser/validator success is not an equivalence proof; the artifact certificate comes from structural emission replay against verified BackendIR.
