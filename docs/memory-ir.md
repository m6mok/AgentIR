# MemoryIR

MemoryIR is a separate typed graph attached to an immutable candidate revision. Scalars remain SSA when storage is unnecessary. Every reachable tensor value has a `MemoryBinding`; every active `MemoryBuffer` records element type, logical shape, exact logical-element strides, layout, abstract address space, access mode, power-of-two alignment, ownership, alias domain, lifetime, external binding, source value, checked offset, and compiler provenance.

`MemoryOperation` retains its source ImplIR operation and ordered operand/result bindings. `BufferAccess` makes high-level typed reads and writes explicit without introducing pointers or a loop schedule. Canonical order follows reachable ImplIR operation order and ordered IDs use memory-local allocators (`mp*`, `mr*`, `buf*`, `mop*`, `ad*`, `mo*`, `mev*`, `mg*`).

`memory_hash` is SHA-256 over domain `agentir.memory.exact.v1\0` and the exact typed plan state. It includes anchors, buffers, bindings, accesses, alias/lifetime facts, decisions, guards/fallbacks, obligations, evidence references, lifecycle and independent codec/semantics/validator versions. It excludes resource policy, timestamps, platform data, diagnostics, benchmark data, and addresses. It never substitutes for `impl_hash` or any legacy hash.
# Schedule boundary

ScheduleIR anchors one fully proved MemoryIR revision and may not edit its buffers, layout, alias/lifetime facts, reuse decisions, or guards. Every schedule verifier rechecks those facts under the proposed order. Schedule revisions therefore preserve `memory_hash`; unsafe reorderings are rejected atomically.

# Backend preservation

Stage 5 maps typed buffers to deterministic WGSL bindings without inventing alias facts. Static reuse becomes a verified read-write binding; guarded reuse retains the compiler-owned `NoOverlap` predicate and exact lazy fresh fallback plan. Device traces and measurements never enter `memory_hash`.
