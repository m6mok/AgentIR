# Pre-Stage-7 readiness

## Freeze inventory

1. Architecture dependency matrix: [pre-stage-7-architecture-audit.md](pre-stage-7-architecture-audit.md).
2. Graph/hash/archive/event contracts: [contract-registry.json](contract-registry.json).
3. Frozen inputs: workspace archives v1–v8, current v9; evaluation archives v1/v2, current v3; pinned legacy fixtures remain immutable.
4. Ownership: core owns compiler graphs/certificates; store owns workspace files; protocol owns transport; policy-eval owns evaluation/ranking/learning.
5. Public API audit: Stage 6C public types have rustdoc and constructors validate before publication.
6. Duplicate/dead-code audit: hashing mechanics remain domain-separated; Stage 6C shares the existing canonical hash helper and does not merge contracts. No new dead visible feature was found.
7. Diagnostics/repair audit: stable learned/cursor/repair error codes and twelve typed repair categories are registered.
8. Resource-limit audit: exact/plus-one tests cover cursor, dataset, training, model, inference, archive, and work limits; limits do not enter semantic/compiler identities.
9. Continuation audit: first/empty/one/exact/plus-one, multi-page, repeated, stale, corrupt, identity, duplicate/loss, and complete/bounded behavior are tested.
10. Learned trust boundary: input allowlist/leakage checks, separate labels, checked fixed-point arithmetic, read-only inference, explicit production dispatch, and non-correctness status are tested.
11. Archive graph: evaluation `v1 → v2 → v3`; workspace `v1 → … → v9`; no cross-family edge.
12. Test inventory: compiler correctness remains in core/store/protocol suites; Stage 6C structural/determinism/mutation/evaluation coverage is in `stage6c.rs` and `contract_registry.rs`; timings remain study-only.
13. Benchmark summary: local study produced 1,704 examples, 142 learned inferences, a 1,048-byte model, and an 18,195,849-byte v3 archive. Two semantic files and archives were byte-identical; timing is observation only.
14. Unresolved warnings: transitive `block v0.1.6` through macOS wgpu 24, with upgrade plan in the architecture audit.
15. Known debt: the cursor facade currently pages exact evaluation choice sets; layer-native production descriptor cursors can adopt the same envelope when enumeration sizes exceed current caps. Study mutation breadth is representative, with deterministic unit coverage carrying additional cases.
16. Safe Stage 7 extensions: new separately versioned evaluation policies, codecs, models, dataset labels, or offline search consumers that preserve production selection and proof boundaries.
17. Contracts Stage 7 cannot change without versioning/migration: every graph/event/archive/hash/ID/cursor/feature/model/diagnostic/proof contract in the registry, plus compiler ordering and production transaction semantics.

## Expensive rejection ordering

| Path | Cheap checks before expensive work |
| --- | --- |
| cursor resume | prefix/version/digest/anchor/limits before page clone |
| dataset import | version/count/hash/source anchors before training |
| model inference | format/hash/schema/codec/dimensions/bytes before dot products |
| archive import | byte/decode/envelope before structural replay; model/dataset relations before publication |
| learned dispatch | inference/model/policy/score validation before one production request |

## Verdict

**Ready for Stage 7.** No unexplained semantic nondeterminism, compiler-hash change, workspace archive change, leakage, replay mismatch, cursor duplicate/loss, unclassified failed study case, correctness bug, or major boundary violation remains. The tracked tree must still be clean after the final commits; raw local analysis stays under ignored `target/`.

## Historical note

This is the freeze record that preceded Stage 7 and is not rewritten as a Stage 7A result. The later narrow implementation and verdict are recorded in [stage-7a-readiness.md](stage-7a-readiness.md).
