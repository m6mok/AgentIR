# Stage 7A scope

Stage 7A is deterministic, bounded, fully offline orchestration over existing compiler-generated menu choice sets. It lives only in `agentir-policy-eval`, uses existing scripted Stage 6B or learned Stage 6C ranking, reconstructs every trajectory in an isolated production evaluation engine, and sends every selected action through the ordinary production verifier and atomic transaction path.

The only algorithm is `deterministic_beam_v1`: level-synchronous beam search with an exact immutable root, checked structural objective, bounded depth/width/children, total ordering, cooperative unit-boundary cancellation, deterministic checkpoints, and exact replay. Search results are recommendations or bounded frontier records; they are not compiler state, correctness evidence, hardware selection, or proof of global optimality.

Stage 7A supports `menu`. Free and hybrid search return `EVALUATION_SEARCH_UNSUPPORTED_SURFACE`; there is no arbitrary-action generator. It has no hardware objective, latency selection, provider inference, network, GPU/device call, stochastic/Bayesian/evolutionary/population search, approximate equivalence, e-graph extraction, live workspace publication, or training during replay. Stage 7B and the rest of Stage 7 remain unspecified.
