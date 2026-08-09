# Evaluation archive v5

New evaluation saves use version 5 and domain `agentir.evaluation.archive.v5\0`. V5 retains all Stage 6A/B/C and frozen Stage 7A records plus cohorts, measured objectives, measured run anchors/work, recommendations, typed eligibility, and replay status.

Evaluation archives v1/v2/v3/v4 are immutable inputs. Loading follows only `v1 → v2 → v3 → v4 → v5`. The pure v4→v5 migration fully verifies v4, records `no_measured_search_history`, and invents no cohort, objective, recommendation, or work. Workspace archive v9 and `HardwareMeasurementRecord` v1 are unchanged.

V5 verification checks envelope and independent hashes, cohort record hashes and same-device/build/config constraints, Stage 7A production replay, measured aggregation/ties, and exact recommendation replay before atomic publication. Hardware work is forbidden during load/replay.
