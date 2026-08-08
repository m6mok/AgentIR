# TargetManifest

TargetManifest is an immutable compiler-owned capability contract. The Stage 4 protocol accepts only a stable profile selector, currently `generic_gpu_v1`; it does not accept arbitrary capability JSON or correctness claims.

The generic profile fixes execution hierarchy limits, subgroup width, supported scalar types and vector widths, abstract address spaces and alignments, resource capacities, and supported schedule operations. IDs are `tm*`, `tmr*`, and `tc*`. Every manifest is sealed at creation and replay-verifiable.

`target_hash` uses the domain `agentir.target.manifest.v1\0` and is distinct from every program, implementation, memory, schedule, and archive hash. Resource-policy changes do not alter it. Target discovery, device drivers, machine binaries, and measured performance are outside Stage 4.
