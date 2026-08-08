# Equivalence and EvidenceIR

Stage 3 adds the separate `MemoryEquivalentToImpl` relation. Fresh bufferization, static reuse, guarded reuse, compositional verification, and sealing allocate compiler-owned memory correctness evidence and obligations. Reference execution and traces are confidence only; no positive test advances memory proof state. Memory evidence is stored independently from candidate EvidenceIR and is anchored to `impl_hash` plus exact input/output `memory_hash`.

Stage 2C still accepts only `EquivalentToSpec`. Approximate `RefinesSpecWithinTolerance` returns `UNSUPPORTED_REFINEMENT`; tolerances and approximate math are deferred.

The correctness chain has this form:

```text
SpecIR(spec_hash)
  == identity ImplIR(hash0)
  == trusted rewrite(hash1)
  == ...
  == current ImplIR(hashN)
```

The first edge is a compiler-owned identity-lowering certificate. Later edges come only from the exact known-rewrite registry. Every edge records its rule, before/after `impl_hash`, targets, discharged side conditions, ImplIR semantics version and correctness EvidenceIR ID. Verification recomputes identity lowering, checks every edge/evidence record and requires the terminal hash to equal current ImplIR. A broken hash, missing evidence, unknown rule or damaged certificate leaves equivalence unproved and blocks load/seal.

Correctness evidence includes identity lowering, known rewrite certificates, canonical-identity validation, recognized production rewrites, guarded rewrite certificates, equality membership paths, explicit materialization provenance and compositional verification. Confidence evidence includes fixed-seed speculative differential/property tests and counterexample search. No observed counterexample increases confidence, but testing never changes an open equivalence obligation to `proved`. A first deterministic counterexample marks the affected debt refuted and the candidate rejected.

Evidence records contain the SpecIR anchor, proposal/obligation where applicable, candidate/revision, input/output implementation hashes, candidate/ImplIR/equality semantics versions, stable validator method/version, normalized parameters, deterministic result, optional first counterexample and compiler provenance. Wall-clock time is absent from semantic and exact candidate hashes. Performance evidence is not present in Stage 2C.

A proposal record is provenance, not EvidenceIR correctness. The proof frontier advances only across an ordered prefix discharged by compiler-owned certificates, including a verified equality path whose endpoints match the next debt item. Guarded exactness composes a conditionally valid primary with a fully proved lazy fallback; agent-supplied certificates, equality edges and guards are not protocol inputs. See [translation-validation.md](translation-validation.md), [equality-proofs.md](equality-proofs.md) and [guarded-fallback.md](guarded-fallback.md).
# Stage 4 evidence

`ScheduleEquivalentToMemory` advances only through compiler-owned structural certificates for exact coverage, dependency order, fusion, binding, vectorization/unrolling, MemoryIR preservation, and target-resource validity. Reference scheduled execution and any future measurements remain confidence-only and never close a schedule obligation.

# Stage 5 evidence

`BackendEquivalentToSchedule` and `ArtifactEquivalentToBackend` are compiler-owned structural relations. WGSL parsing/validation establishes well-formedness only; reference/device differential execution and hardware measurements remain confidence evidence. Neither device success nor benchmark speed can advance, replace, or rank correctness certificates.
