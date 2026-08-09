# Stage 6C scope

Stage 6C adds a deterministic offline learned-ranking foundation to the separate `agentir-policy-eval` layer. It consumes retained Stage 6A/6B evaluation records, encodes only the exact visible feature schema, trains a bounded integer pairwise linear model, produces checked fixed-point scores for an unchanged exact choice set, and dispatches a choice only through the existing production verifier and atomic transaction path.

The learned layer has no legality, proof, equivalence, success, or artifact-selection authority. Labels, scores, loss, held-out metrics, work counters, replay, and timing are evaluation evidence only. Inference receives neither a label nor future outcomes. Training and replay perform no provider, network, GPU, device, or wall-clock-dependent stopping operation.

Stage 6C introduces independent dataset, example, split, training-configuration, training-run, checkpoint, model, input, and inference hash domains. Evaluation archive v3 retains their provenance and migrates legacy v1 only through v2. Workspace archive v1–v9 and compiler event/hash contracts are unchanged.

Stage 7 is outside this scope. The pre-Stage-7 gate is recorded in [pre-stage-7-readiness.md](pre-stage-7-readiness.md).
