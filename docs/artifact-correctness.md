# Artifact correctness

Two compiler-owned proof relations extend the existing exact frontier:

```text
EquivalentToSchedule
→ BackendEquivalentToSchedule
→ ArtifactEquivalentToBackend
```

The backend certificate binds the selected `schedule_hash`, exact ordered node coverage, buffer/binding map, dispatch/index/remainder mapping, numeric contract, validator version, and `backend_hash`. The artifact certificate binds that `backend_hash`, `compiler_build_hash`, complete module/entry-point/ABI/dispatch package, and `artifact_hash`.

Naga parsing and validation prove only WGSL well-formedness. Reference evaluation, device differential execution, and benchmark measurements are confidence evidence and never close an obligation or advance either relation. The protocol has no fields for client-supplied BackendIR, WGSL, bindings, guards, target capabilities, or certificates.

Archive v9 replay recalculates both exact hashes, verifies certificates and source/manifest consistency, reconstructs backend event DAGs and dependency cursors, and checks allocator continuation before publishing a workspace.

Stage 8A adds a separate direct relation:

```text
EquivalentToSchedule
→ CpuArtifactEquivalentToSchedule
```

The compiler-owned CPU certificate binds the exact Stage 1–4 anchor chain, serial schedule coverage, typed rank-one f32 ABI, ordered scalar bytecode lowering, bounds validation, validator version, and `cpu_artifact_hash`. Safe interpreter execution and comparison with the reference evaluator are confidence observations; neither advances the proof frontier. Archive v10 replay structurally verifies every retained CPU package and event without executing bytecode or acquiring hardware.
