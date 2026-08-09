# Evaluation archive v3

New evaluation saves use `agentir.evaluation.archive` version 3 and domain `agentir.evaluation.archive.v3\0`. V3 adds immutable datasets, splits, training configurations/runs/checkpoints, learned models, exact ranking inputs, inference records, and an explicit learned/unlearned status for every episode.

Evaluation archive v1 and v2 remain immutable inputs. Loading follows only `v1 → v2 → v3`; v1 migration invents no ranking and v2 migration invents no dataset, label, split, model, training run, or inference. Migrated episodes are explicitly unlearned. Workspace archive v1–v9 is unrelated and unchanged.

Before publication the loader verifies the envelope, legacy run/episode/aggregate/ranking contracts, dataset/example/source anchors, group split, configuration, checkpoint, model, policy/model relation, exact input, recomputed fixed-point inference, learned status, and archive hash. Failed load/import is atomic. Replay never trains and never calls a provider, network, GPU, device, or benchmark.

Timing and work counters remain observational. Machine-specific timing is excluded from learned semantic identities; evaluation archive identity still covers the retained evaluation record as a whole.
