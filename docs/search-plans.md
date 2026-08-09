# Search plans

`SearchPlan` v1 independently hashes algorithm/version, menu surface, ranking policy and optional model anchors, objective/root anchors, beam width, semantic depth, children per node, level-synchronous expansion order, duplicate/rejection/repair/terminal policies, total tie rule, and deterministic checkpoint cadence.

`deterministic_beam_v1` expands the current depth in frontier order, validates the existing Stage 6B/6C ranking, retains at most the configured children per parent, then sorts all accepted non-duplicate next-layer nodes and retains the beam width. The total order is: ordered objective vector; higher accumulated fixed-point ranking score; compiler-state observation hash; search-local node ID. Stage 6B ranking ties remain higher score, compiler order, then choice ID.

Algorithmic width/depth/children/cadence are semantic plan fields. Runtime node/edge/checkpoint-byte/engine/request caps are safety limits and enter no search identity.
