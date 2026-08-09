# Autotuning campaign sessions

`AutotuningCampaignSession` v1 retains the exact plan, Stage 7A session and ranker, canonical terminal artifacts, optional Stage 7C plan/session, ordered Stage 7D journals, optional Stage 7B cohort/objective/recommendation, status, stopping reason, trace, work, and final result.

Mutations require the exact `autotuning_campaign_session_hash`. A stale base, corrupt sub-stage record, invalid transition, or exceeded limit leaves the campaign, search, acquisition, recovery journal, measurement store, and compiler-owned allocators unchanged. Indeterminate hardware boundaries remain Stage 7D states; there is no synthetic distributed transaction and no silent retry.

Session and result identities use separate v1 domains. Work counters are operational and excluded from semantic result/session identity. A recommendation remains evaluation data and is never published into a live workspace.
