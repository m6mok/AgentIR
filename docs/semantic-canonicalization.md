# Semantic canonicalization

Stage 1.1 introduced two deliberately different frozen-SpecIR identities, and Stage 1.2 preserves their codecs unchanged. `content_hash` remains the exact, history-sensitive identity of one `Program` snapshot and continues to protect replay and provenance. `spec_hash` identifies the semantic canonical form of a complete frozen specification. The archive envelope has a third value, `archive_hash`, which protects one concrete versioned on-disk encoding.

| Hash | Covers | Excludes | Primary use |
| --- | --- | --- | --- |
| `content_hash` | Full `Program`, including compiler IDs, obligations and provenance | Revision timestamp | exact revision replay |
| `spec_hash` | Versioned reachable computation, external interface, inferred types, constraints and `NumericContract` | compiler IDs, history, provenance, diagnostics, unused internal graph | future ImplIR contract anchor |
| `archive_hash` | Version-specific archive body and snapshot | the `archive_hash` field itself | corruption and codec verification |

None of these values substitutes for another. In particular, archive migration preserves every legacy `content_hash`, computes missing `spec_hash` values for frozen revisions, and produces a new `archive_hash` only when it writes the current v3 archive.

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

There is no e-graph, rewrite prover or optimizer in Stage 1.2. Constraint discharge changes proof state in exact `Program` history, but the semantic codec still excludes obligations and deduplicates canonical constraints. A later ImplIR candidate will cite `spec_hash` as the immutable contract it implements or refines.
