# Trusted translation validation

`candidate.translation_check` runs the compiler-owned validator on ordered proof debt. Agent-supplied rule names, certificates, guards and correctness evidence are never trusted.

The validator has three proof paths:

1. Canonical identity: equal before and after `impl_hash` produces a correctness certificate.
2. Known rewrite recognition: the validator applies each production matcher/transform to the exact parent ImplIR and accepts only an exact result-hash match with all side conditions discharged.
3. Guarded self-division: the exact profile documented in [guarded fallback](guarded-fallback.md) produces a compiler guard and certificate.

If no path applies, a deterministic `unsupported` result is persisted in a new immutable candidate revision, `TRANSLATION_UNSUPPORTED` is reported non-fatally, no correctness evidence is created and the obligation remains unresolved. Repeating the same check is idempotent and returns the persisted result.

Correctness evidence records the `spec_hash`, proposal/obligation, candidate revision, before/after hashes, stable validator ID/version, target, discharged side conditions and optional fallback. Replay reconstructs the event and then verifies proposal hashes, proof-chain continuity, debt, frontier, evidence and candidate hash before publication.
