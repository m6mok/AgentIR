# Learned-ranking inference

Inference accepts one exact `RankingInput`, feature schema, learned model, and `RankingPolicyDescriptor` whose kind is explicitly `learned_linear`. Compatibility checks cover input/model/policy hashes, schema, codec, scale, maximum choices, interaction mode, and escape rights before scoring.

Every visible choice receives one checked signed fixed-point score at scale 1,000,000. Choice identity and order are unchanged. The resulting `RankingDecision::Scores` traverses ordinary `rank_choices` validation and deterministic compiler-order/choice-ID tie resolution. Only the later explicit selection calls the production compiler once. Failed inference publishes no trace or selection and consumes no compiler ID.

`InferenceRecord` retains exact input, choice set, model, policy, scores, deterministic work counters, and independent `inference_hash`. Work counters are excluded from inference identity. Archive replay recomputes exact fixed-point scores without training, provider, network, GPU, device, or benchmark calls.
