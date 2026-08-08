# Stage 4 scope

Stage 4 is complete as an exact scheduling layer above one immutable, proved MemoryIR revision and one immutable compiler-owned TargetManifest. Its deliverables are a separate typed ScheduleIR, conservative serial roots, exact split/tile/remainder transforms, restricted pointwise fusion, hierarchy bindings, verified vectorization and unrolling, a deterministic analytical resource simulator, and reference scheduled execution.

The trust boundary is structural. A client may request a transform, but cannot supply target capabilities, coverage/dependence facts, guards, resource certificates, equivalence certificates, or testing evidence as proof. The core reconstructs iteration domains and dependencies, checks MemoryIR alias/lifetime compatibility and target capacities, and publishes the entire transaction or nothing.

Stage 4 does not contain backend IR, code generation, device execution, target discovery, performance measurement, autotuning, ranking, search policy, or a best-plan selector. Those remain Stage 5 or later. Resource policy is operational and never enters `target_hash` or `schedule_hash`.

Completion mapping: SpecIR is Stage 1; ImplIR/CandidateForest is Stage 2A; proof debt and guarded candidates are Stage 2B; exact equality is Stage 2C; MemoryIR is Stage 3; ScheduleIR, TargetManifest, and the resource simulator are Stage 4; backend lowering begins in Stage 5.
