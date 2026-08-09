# Learned-ranking training

Training uses a bounded deterministic pairwise integer perceptron over train groups. Example order is fixed by the exact configuration seed and example identity. Epochs, examples, features, updates, model bytes, weight magnitude, and work units have explicit limits. Zero epochs is valid; an empty train split rejects. Stopping never depends on time or environment variables.

`TrainingConfiguration`, `TrainingCheckpoint`, and `TrainingRun` have independent hashes. A checkpoint anchors the exact dataset, split, and configuration and records the next epoch, weights, bias, and update count. Resumption rejects corrupt or mismatched checkpoints. Identical dataset, split, configuration, and seed produce byte-identical training run and model artifacts.

Training is a standalone offline evaluation operation. It does not hold or mutate compiler state and is never run during archive replay.
