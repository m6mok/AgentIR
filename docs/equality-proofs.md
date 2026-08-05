# Equality proofs and debt discharge

An equality edge is correctness evidence only because the compiler rebuilds it from the production rewrite registry, verifies its exact side conditions and checks its before/after `impl_hash`. Serialized edges are never trusted on load: archive replay reproduces events and the equality verifier recomputes nodes, matches, descriptors, digests, worklist and state hash.

## Canonical explanation

`equality.explain` rebuilds a root-to-member proof path. The chosen path has minimum edge count; equal-length alternatives are ordered lexicographically by stable proof descriptors rather than allocation accident. The explanation returns ordered edges and a domain-separated path digest. Bounds apply to traversal depth and returned proof edges.

The path proves only positive membership in the root's exact equivalence class. Missing membership means “not proved in this bounded space”, never inequality or refutation.

## Candidate equality check

`candidate.equality_check` may discharge only the next unresolved ordered debt item. The supplied target member must match that item's after-`impl_hash`; the equality root must match its before-`impl_hash`; the space/revision/hash, anchor candidate revision and candidate base revision must all verify exactly. The client selects IDs but cannot supply edges, rule claims, side conditions, path digests or certificates.

On success the core creates correctness EvidenceIR, a translation-validation record and an `EqualityMembershipProof`, advances the proof frontier when the ordered prefix is complete and writes candidate canonical/hash v3 under `agentir.candidate.exact.v3\0`. Candidate v1/v2 bytes and hashes remain immutable. Positive evaluation still cannot close debt.

## Replay boundary

Equality events record the candidate-event cursor on which they depend. Replay interleaves candidate and equality histories at these explicit cursors, reproduces compiler IDs and checks all hashes before publication. A materialization event additionally reproduces the ordinary candidate fork/rewrite events it caused. Corrupted anchors, node hashes, rules, side conditions, edges, evidence links, status/worklist state, event order or equality hashes are rejected even when the outer archive checksum is valid.
