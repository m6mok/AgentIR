# Autotuning campaign replay

Campaign replay accepts no executor. It replays Stage 7A through the production verifier, verifies the canonical terminal set and Stage 7C session/result, replays every retained Stage 7D journal against compiler-owned measurement hashes, reconstructs the Stage 7B objective and recommendation, and verifies campaign plan/session/trace/result hashes and checked work bounds.

Replay performs zero benchmark, device, provider, and network calls. A replay status in evaluation archive v8 is evidence that this exact offline check succeeded; it is not correctness, performance, portability, or exactly-once evidence.

Trace and result identities use `agentir.evaluation.autotuning_campaign_trace.v1\0` and `agentir.evaluation.autotuning_campaign_result.v1\0`.
