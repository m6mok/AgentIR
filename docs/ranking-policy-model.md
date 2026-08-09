# Ranking policy model

`RankingPolicyDescriptor` fixes name/version, scripted or external kind, base free/menu/hybrid surface, exact feature schema, deterministic configuration, fixed-point score representation, tie rule, maximum choices, escape rights, and optional opaque model/prompt provenance. Its independent `ranking_policy_hash` excludes secrets, sessions, timestamps, and latency.

CI baselines are `lexicographic_choice_v1`, `first_progress_choice_v1`, `goal_directed_rule_v1`, `proof_frontier_first_v1`, `min_context_choice_v1`, `seeded_uniform_choice_v1`, and `hybrid_ranked_escape_v1`. They are deterministic controls, not learned policies.

Scores are signed `i64` fixed-point units at scale 1,000,000. NaN and infinity are unrepresentable; magnitude and count are checked. Higher scores rank first, followed by compiler order and stable choice ID. Explicit selection may omit scores but must name an exact visible choice.
