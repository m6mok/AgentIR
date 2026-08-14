# Stage 8A deterministic CPU execution

Stage 8A adds one minimal scalar CPU target, `cpu_scalar_v1`, after a proved
ScheduleIR revision. The compiler emits a versioned portable bytecode package,
verifies its exact anchors and structure offline, and executes it with a safe,
deterministic interpreter. The supported subset is scalar f32 constants,
parameters, addition, multiplication and explicit FMA together with
one-dimensional f32 `map` and `zip_map`, deterministic serial iteration and
compiler-owned bounds validation.

The CPU package is derived compiler output, never canonical program input.
Clients select an immutable schedule and may provide runtime inputs only. They
cannot provide bytecode, bindings, functions, instructions, hashes, execution
plans, results, equivalence flags or certificates. Unsupported types, ranks,
operations, regions or schedules reject atomically before a package is
published.

The exact typed graph fixes floating-point operation boundaries. Explicit
`fma(a,b,c)` performs one fused operation with one rounding; an `add` consuming
a `mul` result performs two separate operations and roundings. They are not
interchangeable for arbitrary `f32`, and agreement on one input is not an
equivalence proof. The [CPU authoring quickstart](protocol.md#cpu-jsonl-authoring-quickstart)
shows how static fixtures preserve this graph and obtain compiler-owned IDs and
hashes through successful prefix runs.

The trusted chain remains compiler owned:

```text
EquivalentToSpec -> EquivalentToImpl -> EquivalentToMemory
-> EquivalentToSchedule -> CpuArtifactEquivalentToSchedule
```

Offline structural validation checks the portable package. CPU execution and
its outputs/counters are observation data and never advance a proof frontier.
Runtime dimensions, values, limits, counters, timings and machine metadata are
excluded from every semantic, compiler and CPU artifact hash.

CPU artifacts require workspace archive v10. Archive v1-v9 remain immutable
legacy inputs and migrate only through the explicit v9->v10 edge, which adds an
empty CPU artifact store and invents no package. Archive replay and verification
rehash and structurally validate retained packages but execute no bytecode.

Stage 8A contains no JIT, LLVM, MLIR, native code generation, native ABI,
dynamic libraries, external processes, raw pointers, `unsafe`, threads, SIMD,
GPU work, autotuning or performance selection. Existing WebGPU/WGSL emission
and optional execution remain unchanged compatibility paths. Real CPU timing
is layered separately by [Stage 8B](stage-8b-scope.md), which does not alter
this artifact or correctness contract; measured recommendations remain out of scope.
