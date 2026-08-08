# Reproducibility manifest

An emitted Stage 5 package is reproducible from one exact tuple:

```text
(spec_hash, impl_hash, memory_hash, target_hash, schedule_hash,
 backend_hash, compiler_build_hash, backend/artifact codec versions)
```

The manifest fixes module order, exact WGSL bytes, entry points, workgroup sizes, binding and parameter ABI, dispatch order, compiler-owned guard branches, outputs, and proof entries. Re-emission from the same tuple returns byte-identical modules and the same `artifact_hash`.

Excluded state includes timestamps, resource limits, host paths, device discovery, fingerprint data, execution traces/results, measurements, native pipeline caches, and driver binaries. Archive v9 stores portable WGSL packages only. The v9 generator normalizes revision timestamps and its complete valid/corrupted corpus is byte-identical across consecutive runs.
