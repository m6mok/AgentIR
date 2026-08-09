# Autotuning campaign checkpoints

`AutotuningCampaignCheckpoint` v1 is a complete campaign snapshot with exact plan/session anchors plus the current Stage 7A, optional Stage 7C, and optional Stage 7D checkpoints. Checkpoint creation accepts no executor and performs no hardware work.

Resume verifies the checkpoint prefix, version, independent digest, campaign anchors, search replay state, acquisition slot state, recovery journal, server-owned catalog, and retained measurement store. Ordinary resume never benchmarks and does not silently rerun an indeterminate attempt. Checkpoint byte limits are operational and do not change campaign hashes.

The independent domain is `agentir.evaluation.autotuning_campaign_checkpoint.v1\0`.
