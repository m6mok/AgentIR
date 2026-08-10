# Stage 8 offline closure gate

Stage 8 closes through one fast offline integration gate over the already frozen Stage 8A and Stage 8B contracts. Stage 8C adds no compiler graph, persisted state, archive version, hash domain, protocol mutation, target profile, bytecode opcode, benchmark algorithm, ranking policy, or publication authority.

## Closure evidence

The gate builds the production chain from frozen SpecIR through proved ImplIR and MemoryIR, verified `cpu_scalar_v1` ScheduleIR, and a compiler-published portable CPU package. It then checks four distinct evidence classes:

- Compiler correctness evidence is the existing compiler-owned chain through `CpuArtifactEquivalentToSchedule`. Structural lowering, certificates, exact anchors, and artifact replay establish this relation; execution and timing do not.
- Deterministic CPU execution evidence is exact bounded Stage 8A interpreter output. The closure SAXPY result is `[12.0,24.0,36.0,48.0]`, with the CPU artifact hash and canonical bytes unchanged by execution or measurement.
- Synthetic measurement orchestration evidence uses explicit clock and execution doubles in an isolated workspace/store. It checks exact call counts, integer samples and aggregates, all independent record anchors, atomic rejection, structural query/check, corruption rejection, and archive v11 replay.
- Real monotonic timing is a one-iteration production smoke observation. It asserts provenance and output only, with no duration threshold or comparison.

Archive v11 save/load structurally verifies the retained CPU artifact and measurement without accepting an executor or clock. The pure v10→v11 migration still adds only an empty `CpuMeasurementStore`, and the legacy fixture corpus remains byte-pinned by the existing store tests.

## Explicitly unproved claims

Stage 8 closure does not prove speed, speedup, statistical significance, portability, superiority, global optimality, native-code quality, SIMD effectiveness, CPU/GPU comparability, or exactly-once physical execution. Measurement remains non-correctness observation data and cannot rank, recommend, select, or publish an artifact.

The absence of SIMD, threads, JIT/AOT/native code, LLVM/MLIR, reductions, broader dtype/rank support, and ranking does not block this closure. Reopening Stage 8 or making physical timing an acceptance authority requires a new ADR.
