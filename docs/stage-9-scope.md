# Stage 9 scope: isolated native CPU execution

Stage 9 adds an optional native JIT execution path for an unchanged,
compiler-published Stage 8A `cpu_scalar_v1` package. It does not publish native
machine code, replace the portable package, widen the supported program subset,
or grant execution and timing any correctness or selection authority.

This is the normative implementation plan selected by ADR-184. Stage 9 is
complete only while the closure gate in this document passes.

**Implementation status.** Stage 9A, the bounded Stage 9B production
`cpu_native.execute` contract, and the Stage 9C offline closure gate are
implemented. The gate passes on macOS/aarch64. Linux/x86_64 portability is not
claimed until the same command passes on that target.

## Why this is the next boundary

Stage 8 already provides the right stable input to native compilation: a
bounded, structurally verified, content-addressed scalar package with explicit
f32 addition, multiplication and fused multiply-add semantics. Lowering from
ScheduleIR a second time would duplicate a trusted compiler path and create a
second opportunity for anchor drift. Stage 9 therefore compiles the retained
Stage 8A package directly and never mutates, replaces or re-lowers it.

The first native step is deliberately JIT-only and ephemeral. Persisting object
files or executables would also require a frozen native ABI, linker identity,
object-format normalization, relocation policy, loader security contract and
archive migration. None is needed to prove the smaller execution boundary.

## Alternatives considered

| Option | Useful property | Reason not selected for Stage 9 |
| --- | --- | --- |
| In-process Cranelift JIT | Small direct lowering and native scalar FMA | Calling generated code requires an unsafe host ABI and a code-generation fault could corrupt the protocol process. |
| Out-of-process Cranelift JIT | Direct lowering, native scalar FMA and process-level crash containment | Selected, with one fresh worker per explicit execution. |
| Cranelift AOT object | Publishable target-native bytes | Requires linker, loader, relocation, ABI and reproducibility contracts before the execution boundary is known to be sound. |
| WebAssembly plus Wasmtime | Safe typed embedding, linear-memory isolation and fuel | Core Wasm has no exact scalar fused-multiply-add instruction; relaxed SIMD multiply-add is not an exact substitute. A second portable artifact would also duplicate Stage 8A bytecode. |
| LLVM/MLIR JIT or AOT | Mature optimization surface | Adds a substantially larger versioned toolchain, FFI surface and optimization contract than the first native slice needs. |
| SIMD, threads, reductions or broader types | More throughput or semantic coverage | Each changes a separate target, scheduling or numeric contract and would hide faults in the new native boundary. |

The worker process is crash containment, not an operating-system security
sandbox. It runs with the invoking user's authority. This limited claim is
acceptable only because clients cannot supply code, Cranelift IR, symbols,
target flags or machine bytes; the worker consumes one independently verified
compiler-owned package and ordinary bounded inputs.

## Selected architecture

```text
JSONL cpu_native.execute
  -> agentir-protocol resolves exact retained CPU artifact ID/hash
  -> agentir-runtime-native-cpu validates inputs and projected work
  -> fresh internal worker process with bounded framed request
     -> verify canonical CpuArtifactPackage again
     -> compiler-owned CpuInstruction -> Cranelift IR lowering
     -> Cranelift verifier and fixed server-owned ISA settings
     -> one audited native-call bridge
     -> bounded output response and immediate worker exit
  -> parent validates response, output shape/finiteness and identities
  -> observation response; workspace remains unchanged
```

The existing `CpuArtifactPackage` remains the only published executable CPU
artifact and the Stage 8A interpreter remains the exact portable oracle. Native
machine code, Cranelift IR, relocation state and executable memory are worker
local, ephemeral and absent from snapshots, archives and hashes.

## Stage 9A: native lowering and fixed ABI

Stage 9A introduces one pinned Cranelift family compatible with the repository
Rust 1.85 MSRV. Before implementation proceeds beyond a spike, it must compile
and execute the retained SAXPY package on the primary macOS/aarch64 laptop and
the Linux/x86_64 CI target. A failure of exact FMA lowering, target support or
the MSRV check stops the stage and requires an ADR amendment; it must not be
worked around with separate multiply and add.

The lowering accepts only the current verified `CpuInstruction` set. Every
instruction, function, binding, register, extent and output must be covered
exactly once. The worker runs the Cranelift verifier before finalization and
rejects unresolved functions, data symbols, imports, libcalls and client- or
environment-selected compiler settings.

The JIT entry point uses one versioned internal ABI over worker-owned packed
buffers and checked binding metadata. Only one small module in the worker may
contain local `unsafe` needed to convert and call the finalized entry point.
That module must:

- contain no allocation, parsing, hashing, target selection or lowering;
- validate the ABI version and all pointers, lengths, counts and alignments in
  safe code before the call;
- keep every backing allocation alive for the complete call;
- expose no raw pointer through core, protocol or public JSON;
- document every unsafe precondition next to the operation;
- compile under `deny(unsafe_op_in_unsafe_fn)` and a crate-local unsafe audit
  test.

All existing crates retain `unsafe_code = "forbid"`. The worker package is the
only workspace exception and must not be linked into `agentir-core`,
`agentir-store`, either existing CPU crate, or `agentir-protocol`. The ordinary
CLI process may enter worker mode only from a hidden process argument chosen by
the server-owned launcher before JSONL processing; no JSONL request can select
worker mode.

## Stage 9B: bounded production execution

**Implemented.** `agentir-runtime-native-cpu` is the safe parent runtime and
contains the single shared internal wire/identity/launcher contract. Production
uses a hidden mode of the current `agentir` executable, selected from a
server-owned argument before JSONL processing. The protocol crate depends only
on the safe parent runtime; the CLI and worker package own the child entry and
Cranelift dependency. Tests inject a launcher only through an explicit Engine
constructor and retain direct structural call/request evidence.

The only production command that may compile or execute native code is
`cpu_native.execute`. The request contains only:

- workspace ID;
- retained CPU artifact ID and exact `cpu_artifact_hash`;
- ordinary runtime inputs;
- interactive resource limits already owned by the server.

It contains no bytecode, Cranelift IR, native ABI, target triple, CPU features,
optimization flags, symbols, machine bytes, output, hashes, counters,
certificates, worker path, timeout, environment or success claim.

The launcher starts one fresh worker for one request, clears inherited
environment configuration, uses only bounded stdin/stdout/stderr framing, and
accepts exactly one complete response before process exit. The executable path,
worker mode, target ISA detection, Cranelift settings and safety timeout are
server owned. The timeout is resource policy only: it may be consulted solely
inside `cpu_native.execute`, is excluded from all identities, and is never
reported as a duration or used as performance evidence.

Worker crash, signal, timeout, extra output, truncated output, wrong protocol
version, wrong package or runtime identity, malformed values, non-finite output
or output/hash mismatch is a typed rejection. There is no silent interpreter
fallback and no automatic retry. Native execution is an observation and
publishes no revision, artifact, measurement, ID or event, so every failure is
workspace-atomic.

The production launcher and a test-only worker double share a narrow internal
trait. The double is constructible only through an explicit injected Engine in
tests and is unreachable from JSONL. Tests use a separate Engine/workspace and
assert calls at the launcher boundary; global counters and post-hoc counters
owned by structurally unreachable objects are not acceptable evidence.

## Identity and authority

Stage 9 adds two observation-only domains:

- `cpu_native_runtime_hash` binds the worker protocol version, AgentIR native
  runtime build, exact Cranelift version, target triple, detected ISA features
  actually enabled, fixed code-generation settings and internal ABI version;
- `cpu_native_execution_hash` binds the exact Stage 8A artifact/build hashes,
  `cpu_native_runtime_hash`, existing canonical CPU input hash, projected work,
  exact output shape and output hash.

Paths, PIDs, environment, timestamps, timeout policy, resource limits, process
startup, compile time and execution duration enter neither domain. These hashes
identify observations only. They are not compiler hashes, artifact hashes,
proof certificates, persisted measurement records or ranking inputs.

Stage 9 adds no proof relation. Cranelift IR verification establishes structural
well-formedness, not equivalence to SpecIR. Exact comparison with the Stage 8A
interpreter is confidence evidence only and cannot advance a proof frontier,
publish an artifact, select an executor or prove speed.

## Persistence and compatibility

Stage 9 adds no persistent store, workspace archive version, evaluation archive
version or migration. Workspace archive v11 remains the current writer. Native
execution results and ephemeral machine code are not replayed.

Archive v1-v11 codecs, fixtures and hashes; Stage 1-8 event semantics and hash
domains; `cpu_scalar_v1`; CPU artifact format v1; CPU measurement records;
WGSL/WebGPU contracts; and Stage 6/7 evaluation contracts remain unchanged.
Loading, migration, replay, `list`, `query`, `check`, ordinary CPU execution and
CPU measurement accept no native launcher and perform zero worker starts,
native compilations, native executions and native timeout-clock reads.

## Stage 9C closure gate

Stage 9 closes only while the following fast offline gate passes:

```bash
cargo test -p agentir-native-cpu-worker --test stage9_closure
```

The gate proves the following evidence:

1. The production SpecIR -> ImplIR -> MemoryIR -> ScheduleIR -> Stage 8A CPU
   artifact chain is reconstructed without a second lowering from ScheduleIR.
2. Native execution consumes the exact retained package and leaves its hash and
   canonical bytes unchanged.
3. Native SAXPY output is exactly `[12.0,24.0,36.0,48.0]` and bitwise equal to
   the unchanged Stage 8A interpreter output.
4. A fixed-seed bounded corpus covers every current CPU instruction, scalar and
   tensor bindings, zero/one/multiple extents, signed zero, finite edge values,
   explicit FMA and rejected non-finite results.
5. Malformed packages, stale anchors, bad inputs, projected-work overflow,
   forged worker responses, crashes, timeouts and protocol corruption reject
   without workspace mutation or persistent ID/event consumption.
6. Production JSONL cannot supply or reach the test worker, hidden worker mode,
   compiler flags, ISA selection, ABI, IR, symbols, machine bytes or result
   identities.
7. Structural and archive paths report exactly zero native launches,
   compilations, executions and timeout-clock reads.
8. No worker process remains after success or any tested failure.
9. Archive v1-v11 fixtures and every Stage 1-8 contract remain byte-pinned.
10. The gate contains no duration threshold, interpreter/native comparison for
    performance, speedup assertion, ranking, recommendation or publication.

The closure report must separately state what was proved by structural checks,
what was observed by differential execution, and what remains unproved. A real
native smoke run is compatibility evidence only and may be skipped on an
unsupported host without weakening the offline double-based orchestration gate;
at least macOS/aarch64 and Linux/x86_64 must pass before Stage 9 is declared
portable across those two targets.

The checked closure report is generated as a local reproducibility artifact at
`target/stage9-closure/report.md`. On the recorded macOS/aarch64 host the gate
executes the real worker and exact SAXPY/bitwise corpus. The report deliberately
keeps Linux/x86_64 portability open pending an independent run of the same
command.

## Explicit non-goals

Stage 9 contains no AOT object or executable publication, dynamic library,
stable public native ABI, machine-code cache, SIMD/vector lowering, threading,
reductions, broader dtype/rank support, CPU/GPU comparison, benchmark store,
statistical inference, autotuning, ranking, selection, live publication,
remote worker or exactly-once execution claim.

The next contracts should be chosen from evidence rather than bundled into this
stage: persistent AOT artifacts and host embedding, semantic expansion, explicit
SIMD/thread scheduling, and measurement-aware CPU/GPU policy remain independent
future work.

## Research basis

The decision was checked against the current AgentIR Stage 8 contracts and the
following primary project documentation on 2026-08-11:

- [Cranelift project](https://cranelift.dev/) and
  [`JITModule`](https://docs.rs/cranelift-jit/latest/cranelift_jit/struct.JITModule.html);
- [`ObjectModule`](https://docs.rs/cranelift-object/latest/cranelift_object/struct.ObjectModule.html)
  for the deliberately deferred AOT alternative;
- [Wasmtime interruption](https://docs.wasmtime.dev/examples-interrupting-wasm.html)
  and [deterministic execution](https://docs.wasmtime.dev/examples-deterministic-wasm-execution.html)
  guidance;
- [WebAssembly numeric semantics](https://webassembly.github.io/spec/core/exec/numerics.html),
  especially implementation-dependent NaN and relaxed-operation behavior;
- [LLVM ORC design](https://llvm.org/docs/ORCv2.html) for the deliberately
  deferred LLVM JIT alternative.

Exact crate versions and settings are not selected by this planning document.
The Stage 9A compatibility spike must pin them in `Cargo.lock`, record them in
the runtime identity and demonstrate the repository MSRV before production code
is accepted.
