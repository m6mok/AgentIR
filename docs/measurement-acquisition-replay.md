# Measurement acquisition replay

Replay rehashes the plan, checkpoint, trace, result and every referenced completed production-format record; checks canonical slot order, anchors, counts and duplicate absence; and reproduces the terminal result hash and work contract.

The replay API has no acquisition executor parameter. Benchmark, adapter, device, provider, network and training calls are exactly zero. Frozen timing observations are verified, not remeasured. Missing, corrupt, duplicate or stale records reject before replay publication.
