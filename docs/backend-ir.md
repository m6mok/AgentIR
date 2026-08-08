# BackendIR

BackendIR is a typed executable graph anchored to one immutable `spec_hash`, `impl_hash`, `memory_hash`, `target_hash` and `schedule_hash`. It is never reconstructed from WGSL text.

A backend program owns compiler-assigned kernels, typed SSA expressions, statements, storage bindings, a deterministic uniform ABI, dispatches, output mappings and an optional compiler-owned `NoOverlap` branch. Kernel grouping follows verified ScheduleIR fusion groups. Serial roots become one conservative serial loop; grid/workgroup schedules use global invocation indices and compiler-owned bounds predicates.

`backend_hash` uses the `agentir.backend.wgsl.exact.v1\0` domain and covers the complete typed backend state, anchors, lifecycle and proof data. It excludes runtime limits, devices and measurements. The verifier checks exact schedule-node coverage, binding consistency, dispatch order, guard branches, supported vector widths and the `BackendEquivalentToSchedule` certificate.
