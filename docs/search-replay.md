# Search replay

Replay starts from the exact immutable root in fresh isolated engines. It rebuilds every choice set, reruns scripted scoring or fixed-point learned inference, validates ordinary Stage 6B ties, replays every selected production action, recomputes objective vectors/frontier retention/stopping, and verifies node, edge, trace, result and checkpoint hashes.

Replay never trains, trusts retained path scores, calls a provider/network/GPU/device/benchmark, or uses wall time. A changed rank, choice, compiler outcome, objective, duplicate representative, frontier order, stopping reason, model or digest returns a structured mismatch and publishes nothing.
