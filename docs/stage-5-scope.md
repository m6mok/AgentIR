# Stage 5 scope

Stage 5 adds one exact backend family, `webgpu_wgsl_v1`, after a proved ScheduleIR revision. It lowers the supported one-dimensional elementwise subset (`map` and `zip_map`, including exact arithmetic, comparisons, selection, casts and explicit FMA) into a separate typed BackendIR, verifies structural equivalence to ScheduleIR, and emits deterministic WGSL packages.

The trusted correctness chain is:

```text
EquivalentToSpec → EquivalentToImpl → EquivalentToMemory
→ EquivalentToSchedule → BackendEquivalentToSchedule
→ ArtifactEquivalentToBackend
```

Offline WGSL parsing/validation, device execution and hardware measurements are evidence about well-formedness or confidence. They never create or strengthen a correctness certificate.

Stage 5 excludes reductions, non-contiguous layouts, non-global storage, subgroups, shared memory, atomics, textures, target lowering beyond WGSL, autotuning, ranking and best-artifact selection. Unsupported lowering is a structured atomic rejection and consumes no backend or artifact IDs.

Archive/snapshot v9 adds backend, artifact and measurement stores. Versions v1–v8 remain immutable inputs and migrate through the explicit v8→v9 edge.
