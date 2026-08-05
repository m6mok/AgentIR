# Guarded memory reuse

Stage 3 has exactly one runtime memory predicate: compiler-built `NoOverlap(buffer_a,buffer_b)`. Protocol clients name desired buffers; they cannot submit predicate syntax, alias proofs, lifetime proofs, certificates, or a general boolean guard.

The true branch is a fully verified in-place storage decision. The false branch is an immutable fresh result template with `lazy_fresh_allocation`; it is allocated only after a false outcome. Both branches retain the same `impl_hash`, interface, numeric contract, and observable outputs. Guard metadata is bounded to typed offsets, extents, strides and element type, and fallback graphs are depth-bounded and acyclic.

Reference evaluation runs the anchored ImplIR semantic oracle and emits a deterministic high-level memory trace. An explicit runtime outcome selects either guarded reuse or fallback allocation; the unselected physical branch is absent from the trace. This testing is confidence evidence and never creates or replaces structural correctness evidence.
