# Search graph

`SearchNode` and `SearchEdge` retain exact parent provenance, depth, production-derived compiler-state observation, objective vector, ranking trace/inference anchor, selected visible choice, and production outcome. Independent domains are `agentir.evaluation.search_node.v1\0` and `agentir.evaluation.search_edge.v1\0`.

Search-local IDs use `search-node-N` and `search-edge-N`; they cannot be confused with compiler persistent IDs and depend on deterministic publication order only. Rejected expansion records one edge and leaf, never advances that branch, and relies on production atomicity for unchanged compiler state.

The v1 duplicate policy preserves every alternative parent as its own node/edge and marks later equal compiler states with `duplicate_of` the first total publication representative. Duplicate branches do not re-enter the frontier. Nothing is silently merged and provenance is retained.
