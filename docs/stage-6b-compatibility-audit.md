# Stage 6B compatibility audit

Stage 6B started from clean commit `5eeaa974df42b2923e3b2df9eadcd51a3b8503d1` (`feat: add Stage 6A policy evaluation harness`). Workspace archive v1–v9, compiler event semantics, compiler hash domains, benchmark schema v2, and the 53 committed v8/v9 fixtures remain outside the change.

All ten pinned Stage 6A episode hashes remain byte-identical. Existing evaluation corpus/policy/observation/episode v1 identities remain valid: optional Stage 6B fields are omitted for unranked records, and ranked episodes use the explicit episode v2 transcript domain. Evaluation archive v1 is accepted only under its original domain and migrates explicitly to v2 without ranking invention.

Stage 6B adds only `agentir.evaluation.choice_id.v1`, `choice_set.v1`, `feature_schema.v1`, `ranking_policy.v1`, `ranking_trace.v1`, `selection.v1`, ranked episode v2, and evaluation archive v2 domains. Ranking limits enter none of the compiler or workspace identities. Stage 6B.1 gives semantic choices their own transport-independent domain; this deliberately changes pre-6B.1 evaluation transcripts while leaving every compiler and workspace identity untouched.
