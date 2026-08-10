# Evaluation archive v8

Evaluation archive v8 is separate from workspace archive v11. New evaluation saves use v8 and retain campaign plans, sessions, checkpoints, traces, results, stopping reasons through sessions/results, work counters, exact embedded Stage 7A–7D references, and zero-device replay statuses.

The independent envelope domain is `agentir.evaluation.archive.v8\0`. V1–v7 remain immutable legacy inputs. Loading follows only `v1→v2→v3→v4→v5→v6→v7→v8`. The pure v7→v8 migration first verifies v7, sets `NoCampaignHistory`, creates no synthetic search/acquisition/recovery/cohort/campaign data, and preserves every older record.

V8 verification checks all campaign hashes and flattened stores without opening a device. Workspace archive versions and `HardwareMeasurementRecord` v1 are unchanged.
