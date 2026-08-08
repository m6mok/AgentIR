# ImplIR through Stage 2C

Stage 3 does not add buffers or mutation to ImplIR. A memory plan anchors one immutable `impl_hash` and references reachable `ImplOperation`/`ImplValue` identities from a separate `MemoryProgram`. Equality members must first be explicitly materialized into CandidateForest before they can become MemoryIR roots.

ImplIR is a separate functional typed graph describing one implementation of a frozen SpecIR contract. It reuses the implemented pure opcode/type semantics, but its operations (`iop*`), values (`iv*`), regions, outputs and source links are distinct Rust types. It is not a `Program` alias and no flag turns SpecIR into ImplIR.

`candidate.create` accepts only a complete frozen SpecIR revision with a verified `spec_hash`. Identity lowering preserves parameter/output names, ordered operands, regions, inferred types, accepted constraints and `NumericContract`; it omits SpecIR proof obligations and assigns independent IDs. Source links retain compiler-owned SpecIR provenance.

The ImplIR verifier checks SSA uniqueness and dependency order, references, arity/inference, scalar constants, region arguments/captures/local SSA/yield, external interfaces, source links and resource budgets. An internal adapter lets the reference evaluator share the already-defined pure opcode semantics without collapsing the two data models.

Stage 2B does not change ImplIR semantics or its hash. Proposal fragments are normalized and checked through the same verifier-owned type inference before compiler IDs are allocated. The accepted replacement rewires uses of one result; the old target may remain unreachable and is ignored by `impl_hash` until a trusted prune rule removes it. Guard and fallback remain candidate-level state so lazy control flow does not enter ImplIR v1.

Stage 2C likewise leaves ImplIR semantics/hash v1 unchanged. Equality nodes store verified whole `ImplProgram` snapshots and use `impl_hash` only as their semantic hash-cons key; equality state, proof edges and worklists remain separate metadata.

## `impl_hash`

Canonical version 1 uses domain `agentir.impl.semantic.v1\0`. It alpha-normalizes the output-reachable typed graph, dimensions and region locals. External names, types, ordered operands, regions, constraints and `NumericContract` participate. ImplIR IDs, candidate IDs, source/rewrite provenance, evidence and unreachable nodes do not.

`IMPL_SEMANTICS_VERSION = 1` identifies verifier/evaluator behavior; it is independent of the canonical version, candidate semantics, SpecIR event semantics and archive format.

ImplIR still has no buffers, address spaces, layouts, raw pointers, schedules, threads, tiles, targets or backend instructions.
# Schedule boundary

Stage 4 does not annotate or rewrite ImplIR. Schedule nodes refer to immutable ImplIR operations while iteration order, fusion, hierarchy binding, vectorization, and unrolling live only in ScheduleIR. Consequently schedule edits preserve `impl_hash` exactly.

# Backend boundary

Stage 5 BackendIR retains immutable `impl_hash` and source-operation coverage but never adds WGSL, bindings, dispatches, or device data to ImplIR. Backend lowering cannot change algorithmic/numeric semantics or recalculate `impl_hash`.
