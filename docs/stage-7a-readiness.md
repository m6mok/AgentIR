# Stage 7A readiness

## Audit

- Dependency: search exists only in `agentir-policy-eval`; core/protocol compiler crates have no evaluation dependency.
- Contracts: objective, plan, node, edge, checkpoint, trace, result and repair use distinct v1 domains; workspace/compiler/Stage 6 identities are unchanged.
- Legacy: evaluation v1/v2/v3 verify as immutable inputs and migrate explicitly through v3→v4 without synthetic search. Workspace remains v9.
- Trust boundary: all selected actions traverse production decoding, verification and atomic transactions; ranking/search/task success create no proof.
- Isolation: every branch is reconstructed in a fresh evaluation harness from its exact path; caller/live workspaces are never exploration state.
- Determinism: BTree collections, checked integers, level-synchronous ordering, explicit ties, deterministic local IDs, semantic work horizons and no timing/thread/random stopping.
- Checkpoint: prefix/digest/anchors/graph/frontier/cursor verify before execution; single-shot and split resume are byte-equivalent.
- Archive: v4 validates structure before full production replay and publishes atomically.
- Fairness: exact common anchors/envelope required; failures and no-terminal runs remain visible; timing is observation.
- Study: scripted and learned beam widths 1/2/4 produced 18 searches over a documented deterministic three-task subset: 204 nodes, 186 edges, 168 parent expansions, 18 compiler rejections, 6 policy prunes, 18 duplicate states, 84 learned inferences, 12 terminal and 6 no-terminal runs. Both runs had byte-identical semantics, results, checkpoints, mutations and 18,917,054-byte archives.

## Known debt and Stage 7B blockers

Stage 7A is menu-only and uses conservative whole-trajectory branch reconstruction. It does not persist arbitrary intermediate user-requested checkpoint chains in one run summary, generate free/hybrid actions, search hardware objectives, rank artifacts by latency, run concurrent branches, or claim global optimality. Stage 7B requires a separately frozen scope for broader continuation-native engine snapshots, hardware/objective contracts, statistical design, and any new surface or algorithm.

## Verdict

**Ready for the narrow Stage 7A contract**, subject to the repository quality gate and byte-identical two-run study evidence. This verdict does not complete all of Stage 7.
