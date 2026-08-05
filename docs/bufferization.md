# Verified bufferization

`memory.create` accepts only an explicit fully proved unconditional candidate revision. The core rechecks the frozen SpecIR anchor, candidate proof chain, `spec_hash`, `impl_hash`, ImplIR verifier, reachable order, and resource bounds before allocating any memory-local persistent ID.

The deterministic root gives tensor parameters read-only external regions, tensor constants immutable constant regions, and every reachable tensor result a distinct writable plan-owned contiguous row-major region. Scalar values stay SSA. Fresh result templates remain immutable exact baselines even after a plan selects reuse.

`request_in_place_reuse(input,result)` is accepted only when the core proves identical tensor type/shape, compatible layout/strides/alignment, writable plan ownership, last use at the overwrite point, no output escape of the old value, and no live overlapping reader. The transaction stages the complete store and allocator, rebuilds analysis, verifies `MemoryEquivalentToImpl`, and publishes exactly one revision. Failure moves no head and consumes no ID.
