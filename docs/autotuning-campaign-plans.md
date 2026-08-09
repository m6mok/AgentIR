# Autotuning campaign plans

`AutotuningCampaignPlan` v1 freezes the corpus, task, initial root, Stage 7A objective and plan, ranking policy and optional learned model, benchmark configuration, record count, validation policy, acquisition checkpoint cadence, Stage 7D recovery policy, Stage 7B metric and aggregation, indifference band, terminal policy, and explicit terminal-artifact cap.

The v1 terminal policy retains every distinct compiler-published artifact reachable from a completed or bounded terminal path that is present, proved/offline-valid, and validated in the server-owned catalog. Artifact hashes are sorted and deduplicated before the operational cap is applied. Timing never participates. An empty set produces a typed `NoEligibleTerminal` result and never starts acquisition.

The independent identity domain is `agentir.evaluation.autotuning_campaign_plan.v1\0`. Runtime limits, paths, request IDs, timestamps, wall clock, and work counters are excluded.
