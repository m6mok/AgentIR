# Stage 3 scope: exact MemoryIR

Stage 3 is complete at the logical-to-physical storage boundary. A `MemoryPlanStore` contains independent immutable `mp*` plans and `mr*` revisions anchored to one frozen `spec_hash` and one fully proved unconditional candidate `impl_hash`. Creating or editing a plan never mutates SpecIR, CandidateForest, EqualityStore, or their allocators.

The implemented surface is deterministic fresh bufferization, typed abstract regions, layouts and strides, address spaces, ownership/access/alignment, compiler-owned alias and logical-lifetime analysis, statically proved last-use reuse, the compiler-owned `NoOverlap` guarded profile with an immutable lazy fresh fallback, structural `MemoryEquivalentToImpl` evidence, reference evaluation/tracing, continuations, event replay, and archive v7.

MemoryIR preserves high-level operation identity. It has no raw pointers, byte-address arithmetic, pointer casts, loops produced by lowering, ScheduleIR, TargetManifest, device execution, target capacity claim, cost model, ranking, search, approximate refinement, LLVM, or MLIR. Choosing among legal plans is an explicit client policy decision.

Completion mapping is: SpecIR → Stage 1; ImplIR/CandidateForest → Stage 2A; proof debt and guarded candidates → Stage 2B; exact equality space → Stage 2C; logical-to-physical bufferization → Stage 3. ScheduleIR and TargetManifest begin in Stage 4.
