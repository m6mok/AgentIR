# TargetManifest

TargetManifest is an immutable compiler-owned capability contract. The protocol accepts only stable profile selectors (`generic_gpu_v1`, `webgpu_wgsl_v1`, and `cpu_scalar_v1`); it does not accept arbitrary capability JSON or correctness claims.

The generic profile fixes execution hierarchy limits, subgroup width, supported scalar types and vector widths, abstract address spaces and alignments, resource capacities, and supported schedule operations. IDs are `tm*`, `tmr*`, and `tc*`. Every manifest is sealed at creation and replay-verifiable.

`target_hash` uses the domain `agentir.target.manifest.v1\0` and is distinct from every program, implementation, memory, schedule, and archive hash. Resource-policy changes do not alter it. Target discovery, device drivers, machine binaries, and measured performance are outside Stage 4.

# CPU scalar profile

Stage 8A adds immutable `cpu_scalar_v1`: serial execution, rank-one f32 scalar/tensor bindings, vector width one, exact compiler-owned iteration, and no grid/workgroup/subgroup/device capability. Its target identity uses a separate CPU profile domain so every pre-Stage-8A GPU target byte and hash remains unchanged. It is a portable interpreter contract, not a description of the host CPU and not a performance claim.

# WebGPU profile

Stage 5 adds immutable `webgpu_wgsl_v1`: WebGPU compute, WGSL v1, global storage plus uniform parameters, f32/i32/u32 ABI scalars, vector widths 1/2/4, explicit workgroups, runtime bounds checks, and no atomics, subgroups, shared-memory cache, textures, matrices, or vendor instructions. The existing `generic_gpu_v1` bytes and `target_hash` remain unchanged. Adapter discovery validates reported limits against this minimum contract but cannot mutate it.
