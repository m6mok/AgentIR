# Evaluation archive v6

New evaluation saves use version 6 and `agentir.evaluation.archive.v6\0`. V6 retains all unchanged Stage 6, Stage 7A and Stage 7B records plus acquisition plans, stopped sessions, slot-boundary checkpoints, traces, results, completed production-format measurement records, replay status and explicit result-to-cohort links.

Evaluation v1-v5 remain immutable legacy inputs. Load follows only `v1→v2→v3→v4→v5→v6`. The pure v5→v6 edge fully verifies v5, records `no_acquisition_history`, and invents no plan, session, result, timing or cohort. Verification is offline and performs zero hardware calls. Workspace archive remains v9 and `HardwareMeasurementRecord` remains v1.
