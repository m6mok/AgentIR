# Evaluation archive v2

New evaluation saves use `agentir.evaluation.archive` version 2 and domain `agentir.evaluation.archive.v2\0`. V2 adds feature schemas, ranking policy descriptors, exact choice sets, per-step ranking/selection records, and an explicit ranked/unranked status for every episode.

Evaluation archive v1 remains an immutable input. `migrate_archive_v1_to_v2` first verifies the exact v1 envelope and transcript, then changes only the envelope version, adds empty ranking stores, and marks every legacy episode `unranked`. It never invents scores, traces, selections, or outcomes. Loading performs the pure migration before ordinary replay.
