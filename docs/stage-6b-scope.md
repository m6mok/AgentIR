# Stage 6B scope

Stage 6B adds replayable policy-owned ranking over bounded compiler-generated continuation choices. The compiler still owns legality, types, proof effects, guards, transactions, outcomes, and task success. A ranking score is an interaction decision only: it cannot legalize an action, prove equivalence, or select an artifact by measured speed.

The implementation extends `agentir-policy-eval`, preserves Stage 1–6A compiler and workspace archive contracts, and adds exact choice sets, visible feature schema v1, fixed-point ranking, explicit selection, ranking replay/metrics/fairness, and evaluation archive v2. It has no learned model, provider SDK, network call, autotuning, hardware-driven mutation, or automatic fastest-artifact selection.

Candidate, memory, schedule, and compatible-hole multi-choice frames are constructed from production continuation responses. A layer with one legal action remains a one-choice frame. Hybrid escape stays outside the compiler choice set and must traverse the ordinary production decoder/verifier/transaction path.
