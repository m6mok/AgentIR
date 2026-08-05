# Proof debt

Every accepted proposal appends one immutable, ordered `ProofDebtItem`. An item connects its before and after `impl_hash`, proposal identity, target and boundary, origin event, allowed compiler discharge methods, ordered evidence and optional first counterexample.

Statuses have deliberately different meanings:

- `open`: no trusted validation result yet;
- `proved`: a compiler-owned exact certificate discharged the item;
- `guarded`: the restricted guard plus proved fallback discharged it;
- `unsupported`: the validator has no applicable trusted path; this is neither proof nor refutation;
- `refuted`: deterministic validation found a counterexample.

The candidate head is the newest accepted history. The proof frontier is the last consecutive trusted prefix and its terminal proved `impl_hash`; it may lag behind the head. Validation cannot skip, reorder or jump across open, unsupported or refuted debt. Multiple speculative steps are bounded and remain in acceptance order.

Positive differential or property testing is confidence evidence only and never advances the frontier. A counterexample refutes the first affected unresolved item, preserves the frontier, rejects the candidate and blocks sealing. Forking preserves the selected revision's debt; recovery uses a proved ancestor before the speculative step.
