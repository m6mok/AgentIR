# Learned-ranking model

The v1 artifact is `pairwise_integer_linear_v1`. Its feature codec, `visible_feature_integer_codec_v1`, maps the ordered visible schema to checked integers and appends compiler order. Text values use a deterministic bounded domain hash projection; lists must be sorted and unique. The artifact retains exact dataset, split, training configuration, feature schema, codec, score scale, choice bound, ordered feature names, weights, bias, and its independent `learned_model_hash`.

Weights, bias, encoded features, updates, dot products, and scores use checked integer arithmetic. Overflow, missing/extra weights, wrong scale, future format, incompatible schema/dataset, corrupt retained hash, and oversized artifacts reject before inference output. Floating-point values, NaN, infinity, native ML runtimes, Python, GPU, network, and provider SDKs are absent.

The model hash is not a ranking policy hash and neither is a compiler hash. Changing a weight changes the model hash but cannot change compiler state or correctness evidence.
