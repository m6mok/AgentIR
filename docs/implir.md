# ImplIR Stage 2A

ImplIR is a separate functional typed graph describing one implementation of a frozen SpecIR contract. It reuses the implemented pure opcode/type semantics, but its operations (`iop*`), values (`iv*`), regions, outputs and source links are distinct Rust types. It is not a `Program` alias and no flag turns SpecIR into ImplIR.

`candidate.create` accepts only a complete frozen SpecIR revision with a verified `spec_hash`. Identity lowering preserves parameter/output names, ordered operands, regions, inferred types, accepted constraints and `NumericContract`; it omits SpecIR proof obligations and assigns independent IDs. Source links retain compiler-owned SpecIR provenance.

The ImplIR verifier checks SSA uniqueness and dependency order, references, arity/inference, scalar constants, region arguments/captures/local SSA/yield, external interfaces, source links and resource budgets. An internal adapter lets the reference evaluator share the already-defined pure opcode semantics without collapsing the two data models.

## `impl_hash`

Canonical version 1 uses domain `agentir.impl.semantic.v1\0`. It alpha-normalizes the output-reachable typed graph, dimensions and region locals. External names, types, ordered operands, regions, constraints and `NumericContract` participate. ImplIR IDs, candidate IDs, source/rewrite provenance, evidence and unreachable nodes do not.

`IMPL_SEMANTICS_VERSION = 1` identifies verifier/evaluator behavior; it is independent of the canonical version, candidate semantics, SpecIR event semantics and archive format.

Stage 2A ImplIR has no buffers, address spaces, layouts, raw pointers, schedules, threads, tiles, targets or backend instructions.
