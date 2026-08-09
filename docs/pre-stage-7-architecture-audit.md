# Pre-Stage-7 architecture audit

## Dependency matrix

| Crate | Owns | Direct project dependencies | Forbidden inbound concern |
| --- | --- | --- | --- |
| `agentir-core` | SpecIR through BackendIR typed graphs, certificates, snapshots | none | transport, filesystem, evaluation, learned model, runtime |
| `agentir-eval` | deterministic CPU reference semantics | core | policy/training authority |
| `agentir-store` | workspace archive v1–v9 I/O and replay | core | evaluation archive |
| `agentir-backend-wgsl` | deterministic WGSL emission/offline validation | core | device execution, ranking |
| `agentir-runtime-wgpu` | optional device execution/measurement | core, backend-wgsl | compiler proof authority |
| `agentir-protocol` | production request/response sessions | core, store, eval, backend/runtime adapters | learned training |
| `agentir-policy-eval` | Stage 6 corpus, ranking, learning, evaluation archive v3 | core, protocol | compiler graph ownership |
| `agentir-cli` | compiler JSONL transport | protocol | evaluation orchestration |
| `agentir-eval-cli` | evaluation JSONL transport | policy-eval | compiler ownership |

`cargo tree` shows no project dependency cycle. Learned training is implemented inside the evaluation crate and is absent from core/protocol/runtime dependencies. The runtime/device dependency remains isolated behind `agentir-runtime-wgpu`; evaluation invokes production protocol but cannot flow back into it.

## Boundary findings

- Filesystem persistence remains in `agentir-store` and the evaluation archive layer; core snapshots/replay are I/O-free.
- Evaluation/ranking/learning owns no compiler graph or persistent compiler ID.
- Backend/runtime types do not flow back into Stage 1–4.
- No new dependency, unsafe block, network client, provider SDK, native ML runtime, Python runtime, or GPU training path was added.
- The existing `block v0.1.6` future-incompatibility warning is transitive through `wgpu 24 → wgpu-hal/metal`; removing it requires a coordinated wgpu/Naga upgrade and MSRV/API validation. It is documented rather than destabilized in this stage.

The full machine-readable contract inventory is [contract-registry.json](contract-registry.json), with duplicate/version/migration/documentation tests in `contract_registry.rs`.
