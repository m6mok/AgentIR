# Backend lowering

`backend.lower` names one immutable ScheduleIR revision and its expected `schedule_hash`. The core verifies the complete SpecIR → ImplIR → MemoryIR → TargetManifest → ScheduleIR anchor chain before the WGSL compiler constructs any persistent ID.

The deterministic lowering order is kernel grouping, MemoryIR buffer-to-binding mapping, scalar parameter ABI construction, dispatch/index derivation, exact remainder and bounds handling, restricted fusion, vector/unroll metadata, and guarded `NoOverlap` branch retention. The resulting typed `BackendProgram` is structurally verified before `backend_hash` and `BackendEquivalentToSchedule` are published atomically.

The conservative serial root emits one dispatch and a fixed-order bounded loop. Grid/workgroup schedules emit `global_invocation_id.x` with a compiler-owned bounds predicate. Unsupported opcodes, layouts, address spaces, vector widths, numeric contracts, targets, or open proof state return `UNSUPPORTED_BACKEND_LOWERING`; the selected legal ScheduleIR remains valid and no backend allocator state changes.

All extent, offset, binding, allocation, and dispatch calculations cross checked or saturating conversions and are bounded by `ResourceLimits` before publication.
