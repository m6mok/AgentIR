# WGSL artifact format

Artifact package format v1 is a typed, deterministic execution package. It contains the immutable Stage 1–4 anchors, `backend_hash`, `compiler_build_hash`, ordered modules and entry points, storage and uniform ABI, dispatch and guard plans, output mappings, offline-validation result, and compiler-owned proof manifest.

WGSL modules are UTF-8 with LF endings, stable declaration/identifier/binding order, stable whitespace, and no timestamp, path, driver cache, native binary, device data, or diagnostic noise. The runtime consumes this package directly and performs no semantic lowering.

`artifact_hash` uses domain `agentir.artifact.wgsl.package.v1\0` over the exact manifest, exact ordered WGSL bytes, ABI, dispatches, validation state, and artifact certificate fields. Device fingerprints, measurements, runtime results, resource policy, filesystem paths, and pipeline caches are excluded.

See [Artifact package](artifact-package.md) for the Rust model and [Reproducibility manifest](reproducibility-manifest.md) for byte-level requirements.

# CPU scalar artifact format

Stage 8A CPU packages use the separate `agentir.cpu.scalar.package` format v1. The package retains exact Stage 1–4 hashes, the CPU target profile, ordered input/output bindings, f32 constants by exact bits, ordered portable scalar instructions, compiler build identity, and a compiler-owned schedule-equivalence certificate. It contains no native code, pointers, host path, timestamp, device fingerprint, timing, runtime resource policy, or persistent ImplIR operation/value IDs.

`cpu_artifact_hash` uses its own domain over canonical package semantics. The compiler derives `cpuart-*` IDs from that hash; the derived ID and hash field are excluded from their own preimage. CPU packages are not WGSL packages and do not alter `backend_hash` or `artifact_hash`.
