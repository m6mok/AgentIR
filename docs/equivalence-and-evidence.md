# Equivalence and EvidenceIR

Stage 2A accepts only `EquivalentToSpec`. Approximate `RefinesSpecWithinTolerance` returns `UNSUPPORTED_REFINEMENT`; tolerances and approximate math are deferred.

The correctness chain has this form:

```text
SpecIR(spec_hash)
  == identity ImplIR(hash0)
  == trusted rewrite(hash1)
  == ...
  == current ImplIR(hashN)
```

The first edge is a compiler-owned identity-lowering certificate. Later edges come only from the exact known-rewrite registry. Every edge records its rule, before/after `impl_hash`, targets, discharged side conditions, ImplIR semantics version and correctness EvidenceIR ID. Verification recomputes identity lowering, checks every edge/evidence record and requires the terminal hash to equal current ImplIR. A broken hash, missing evidence, unknown rule or damaged certificate leaves equivalence unproved and blocks load/seal.

Correctness evidence includes identity lowering, known rewrite certificates and compositional verification. Confidence evidence includes fixed-seed differential/property testing. No observed counterexample increases confidence, but testing never changes an open equivalence obligation to `proved`. A differential counterexample marks the candidate rejected because it contradicts a compiler correctness claim.

Evidence records contain the SpecIR anchor, candidate/revision, input/output implementation hashes, candidate/ImplIR semantics versions, stable method, normalized parameters, deterministic result, optional first counterexample and compiler provenance. Wall-clock time is absent from semantic and exact candidate hashes. Performance evidence is not present in Stage 2A.
