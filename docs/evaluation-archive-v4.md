# Evaluation archive v4

New evaluation saves use version 4 and domain `agentir.evaluation.archive.v4\0`. V4 retains all Stage 6A/B/C records plus exact Stage 7A objectives, plans, run summaries, nodes, edges, final checkpoints, traces, results, typed rejections, work counters and ranker/model anchors.

Evaluation archives v1/v2/v3 are immutable inputs. Loading follows only `v1 → v2 → v3 → v4`. The pure v3→v4 edge verifies v3 first, sets `no_search_history`, and invents no objective, plan, run, node, edge, checkpoint, trace or result. Workspace archive v1–v9 is unrelated and has no cross-family migration.

Verification orders cheap checks before expensive replay: byte/decode/version, envelope hash, counts/uniqueness, hash domains, anchors/model compatibility, graph/frontier/checkpoint structure, production replay, objective/frontier/result recomputation, then final publication. Failed load is atomic.
