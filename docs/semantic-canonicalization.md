# Semantic canonicalization

`memory_hash` is an eighth non-substitutable contract. Its exact v1 codec covers physical MemoryIR state and uses domain `agentir.memory.exact.v1\0`; different layouts or reuse choices deliberately differ while retaining the same `impl_hash`. Resource limits, traces, timing, platform and timestamps are excluded. Existing content/spec/impl/proposal/candidate/equality codecs are unchanged.

Stage 1.1 introduced two deliberately different frozen-SpecIR identities. Stage 2A/2B/2C preserve their codecs and ImplIR v1 unchanged while adding proposal, candidate and equality-layer contracts; the archive envelope remains separate.

| Hash | Covers | Excludes | Primary use |
| --- | --- | --- | --- |
| `content_hash` | Full `Program`, including compiler IDs, obligations and provenance | Revision timestamp | exact revision replay |
| `spec_hash` | Versioned reachable computation, external interface, inferred types, constraints and `NumericContract` | compiler IDs, history, provenance, diagnostics, unused internal graph | future ImplIR contract anchor |
| `impl_hash` | Reachable typed ImplIR semantics, external interface, constraints and `NumericContract` | ImplIR/candidate/evidence IDs, source links, history, unreachable nodes | implementation semantic identity |
| `proposal_hash` | Base implementation, target/boundary, alpha-normalized fragment, output type and `NumericContract` | candidate/revision/evidence and later persistent ImplIR IDs, time and limits | proposal identity before allocation |
| `candidate_hash` v1/v2/v3 | Exact candidate revision, IDs, anchor, ImplIR and proof state; v2 adds proposals/debt/frontier/guard; v3 adds equality proofs/materialization | timestamps and resource policy | candidate replay and provenance |
| `equality_hash` v1 | Anchor, whole-program nodes, trusted edges, worklist and saturation status | equality revision history, timestamps and resource policy | resumable exact equality state |
| `archive_hash` | Version-specific archive body and snapshot | the `archive_hash` field itself | corruption and codec verification |

None substitutes for another. Migration preserves legacy `content_hash`, `spec_hash` and candidate hashes; v4 → v5 adds empty speculative stores and v5 → v6 adds an empty equality store without legacy hash recalculation.

## Semantic canonical form v1

The codec discriminator is `agentir.spec.semantic`, the canonical version is `1`, and the hash is:

```text
sha256("agentir.spec.semantic.v1\0" || canonical_bytes)
```

`canonical_bytes` is compact deterministic JSON over `SemanticCanonicalProgramV1`; it is not `serde_json::to_vec(Program)`.

Parameters are sorted by external name and assigned `p0`, `p1`, and so on. Parameter names and normalized types remain part of the public interface, including unused parameters. Outputs are likewise sorted by name and retain both their names and canonical value expressions.

The node graph is reached only from outputs. A dependency-first traversal follows output order, operand order and region capture uses, assigning `n0`, `n1`, and so on. Persistent operation/value IDs, topological insertion order and unreachable constants or operations are absent. Ordered operands are never commutatively sorted, because floating-point and non-commutative semantics do not justify that rewrite. Shared subgraphs remain shared; duplicating a subgraph is therefore a different canonical graph.

Constants retain their exact typed representation, including IEEE-754 binary32 bits. JSON object attributes are recursively stabilized. A string that looks like an unresolved persistent compiler reference is rejected with `CANONICALIZATION_FAILED` rather than incorporated into a supposedly history-independent hash.

## Alpha-normalization

Symbolic dimensions receive `d0`, `d1`, and so on at first semantic use: sorted parameters, sorted output types, reachable graph traversal and then sorted relevant constraints. Static and one-symbol affine expressions retain their exact coefficient and offset. Constraints disconnected from the interface and reachable graph are omitted, as are unused dimension declarations; relevant non-negative declarations are retained.

Region arguments become `%arg0`, `%arg1`, and region-local results become `%local0`, `%local1` in execution order. Original local names never enter the codec. Actual outer uses become canonical outer value references. An unused entry in the region capture allow-list is not semantic and is omitted.

## Validity and cache checking

Only a well-formed, complete, frozen SpecIR can be canonicalized. Drafts, missing outputs, open holes and open obligations return structured `SPEC_NOT_COMPLETE`. A successful freeze stores both `spec_hash` and `semantic_canonical_version` on the new revision; a fork preserves them.

`program.query` with `view: "semantic_canonical"` recomputes the representation and checks the cached values before returning the model, byte length and hash. Archive replay independently recomputes every frozen revision hash and rejects mismatched cache data.

## Deliberate limits

Semantic canonicalization establishes history independence for the same ordered typed graph. It does not establish algebraic equivalence. These may intentionally hash differently:

- `a + b` and `b + a`;
- shared and duplicated subgraphs;
- `mul` plus `add` and `fma`;
- different reassociations;
- different `NumericContract` values.

Stage 2C adds a deliberately finite compiler-owned equality proof graph, not an algebraic canonicalizer, congruence-closure e-graph or ranked search. Nodes with equal `impl_hash` merge, but the equality layer does not alter any underlying semantic codec. A well-typed proposal can still leave proof debt open until a trusted validator or matching equality path closes it. See [equality-space.md](equality-space.md) and [equivalence-and-evidence.md](equivalence-and-evidence.md).
