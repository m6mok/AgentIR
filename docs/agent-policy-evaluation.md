# Agent policy evaluation

An evaluation run binds one exact `corpus_hash`, `PolicyDescriptor`, compiler build hash, ordered task selection, and deterministic seed set. Each episode owns a fresh production protocol engine. The harness assigns run/episode/step IDs, constructs the observation, validates the observation hash and policy surface, applies the selected action, classifies the compiler response, updates budgets, and evaluates the task criterion.

The shipped deterministic baselines are `free_reference_v1`, `menu_first_valid_v1`, `menu_goal_directed_v1`, `hybrid_menu_preferred_v1`, and `hybrid_bounded_escape_v1`. They are scripted controls, not learned agents. Provider identifiers and prompt hashes may be opaque descriptor provenance; secrets, timestamps, latency, sessions, and provider payloads are excluded.

Evaluation records report interaction efficiency only. An accepted compiler transaction has its ordinary Stage 1–5 meaning; an evaluation success means only that the task's machine-checkable criterion was satisfied.

Stage 6B additionally binds exact visible choice-set and feature-schema hashes to ranked observations. A policy ranks or explicitly selects; the harness validates membership and only then invokes the same production path. See [ranking-policy-model.md](ranking-policy-model.md).
