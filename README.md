# AgentIR

AgentIR — экспериментальная агентно-нативная компиляционная среда для численных вычислений. В ней программа хранится не как исходный текст, а как типизированный граф. Агент меняет граф небольшими атомарными ActionIR-транзакциями, а compiler core выводит типы, проверяет формы и сохраняет каждое принятое состояние как неизменяемую ревизию.

Репозиторий завершил Stage 8 через offline CPU execution closure gate. Exact Stage 1–8A compiler/CPU stack, bounded Stage 8B timing опубликованных `cpu_scalar_v1` packages и workspace archive v11 проверяются совместно без нового persisted state или authority. Timing остаётся non-correctness observation, а не performance/correctness proof, ranking, live publication или global-optimality claim.

## Что уже работает

- типы `bool`, `i32`, `f32`, `index` и `tensor<T,[...]>`;
- static, symbolic и простые affine dimensions;
- `parameter`, `constant`, арифметика, `fma`, `compare`, `select`, `cast`, `map`, `zip_map`, `reduce`;
- чистые regions с block arguments и явными scalar captures;
- атомарные транзакции с `$temporary`, `@short` и persistent references;
- immutable revision DAG, fork и structural diff;
- typed holes, proof obligations и continuation frames;
- deterministic canonical JSON и SHA-256 content hash;
- history-independent semantic canonical form и `spec_hash` замороженного SpecIR;
- CPU reference interpreter;
- компактный deterministic `ConstraintFacts`, который доказывает symbol/static equality и закрывает `ShapeCompatible` obligations;
- event-level compiler semantics v1/v2 для точного replay исторических транзакций;
- workspace archive v11, явная migration v1 → v2 → v3 → v4 → v5 → v6 → v7 → v8 → v9 → v10 → v11, mixed candidate/equality/memory/target/schedule/backend/WGSL/CPU-artifact/CPU-measurement replay;
- централизованные resource budgets для core, evaluator, store, protocol и CLI;
- fixed-seed soundness/mutation corpora и statistical benchmark schema v2;
- stateful JSONL CLI с одним ответом на каждый запрос;
- отдельный typed ImplIR и deterministic identity lowering frozen SpecIR;
- immutable candidate revision DAG с fork, atomic rewrite transactions и seal;
- exact known rewrites: unreachable pruning, identical cast elimination и defined scalar constant folding;
- композиционная цепочка `EquivalentToSpec` и разделение correctness/confidence evidence;
- fixed-seed differential validation SpecIR/ImplIR;
- typed single-operation replacement proposals с explicit speculative opt-in и отдельным `proposal_hash`;
- ordered proof debt и proof frontier, который продвигается только trusted compiler certificates;
- canonical-identity и production-known-rewrite recognition, deterministic refutation;
- exact lazy guarded fallback для `i32 div(x,x) -> 1` при `x != 0`;
- whole-program equality nodes, hash-consed по `impl_hash`, и compiler-owned positive proof edges;
- deterministic bounded expansion/saturation, canonical explanations и continuation;
- equality proof discharge для ordered proof debt и explicit candidate materialization;
- candidate semantics/hash v1/v2/v3 coexistence и immutable archive/snapshot v6 compatibility.
- отдельный typed MemoryIR с immutable memory-plan revisions и `memory_hash`;
- explicit buffers, layouts, strides, address spaces, access/ownership/alignment, alias domains и logical lifetimes;
- deterministic fresh bufferization, proved last-use reuse и compiler-owned `NoOverlap` guard с lazy exact fallback;
- reference MemoryIR evaluation/trace, memory continuations, protocol и archive/snapshot v7.
- immutable `generic_gpu_v1` TargetManifest и независимый `target_hash`;
- typed ScheduleIR с serial root, split/tile/remainder, restricted fusion, hierarchy binding, vectorization и unrolling;
- compiler-owned schedule legality/equivalence evidence, deterministic resource estimates и независимый `schedule_hash`;
- reference scheduled execution, target/schedule protocol, archive/snapshot v8 и exact replay.

## Быстрый старт

Нужен stable Rust 1.85 или новее.

```bash
cargo build --workspace
cargo test --workspace
cargo run -p agentir-cli --bin agentir < examples/saxpy.jsonl
cargo run -p agentir-cli --bin agentir < examples/candidate_identity.jsonl
cargo run -p agentir-cli --bin agentir < examples/candidate_rewrite.jsonl
cargo run -p agentir-cli --bin agentir < examples/speculative_open.jsonl
cargo run -p agentir-cli --bin agentir < examples/speculative_promote.jsonl
cargo run -p agentir-cli --bin agentir < examples/guarded_candidate.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_saturate.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_discharge.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_materialize.jsonl
cargo run -p agentir-cli --bin agentir < examples/memory_fresh.jsonl
cargo run -p agentir-cli --bin agentir < examples/memory_reuse.jsonl
cargo run -p agentir-cli --bin agentir < examples/memory_guarded_reuse.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_to_memory.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_serial.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_tiled.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_remainder.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_fused.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_vectorized.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_guarded_memory.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_to_schedule.jsonl
```

Последний ответ SAXPY содержит:

```json
{"outputs":{"out":[12.0,24.0,36.0,48.0]},"dimensions":{"N":4}}
```

Полный разбор сценария — в [docs/getting-started.md](docs/getting-started.md). Формат команд и ошибок — в [docs/protocol.md](docs/protocol.md).

## Почему это не обычный текстовый язык

Текст и JSON здесь только кодеки. Авторитетное состояние — проверенный SSA-граф с persistent IDs. Клиент не переписывает файл и не дублирует выведенные типы: он указывает структурированное изменение и `base_revision`. Compiler core либо применяет весь набор действий, либо не меняет workspace вообще.

Такая модель делает явными три вещи, которые трудно надёжно удерживать в длинном source-edit loop: частичную программу, пространство допустимых продолжений и происхождение каждого результата.

## Режимы клиента

- `free`: клиент самостоятельно посылает schema-valid ActionIR; core проверяет транзакцию.
- `menu`: клиент выбирает только из compiler-generated continuation frame; escape отключён.
- `hybrid`: hard constraints фильтруют невозможное, soft ranking помогает выбору, speculative escape остаётся доступен.

Все режимы используют один `agentir-core`; различается только политика клиента и содержимое continuation frame.

## Crates

- `agentir-core` — SpecIR/ImplIR/MemoryIR/ScheduleIR/BackendIR, TargetManifest, verifiers, immutable plans, proofs и continuations;
- `agentir-eval` — детерминированные semantic и physical reference interpreters;
- `agentir-backend-wgsl` — deterministic lowering, WGSL package emission и offline Naga validation без device I/O;
- `agentir-runtime-wgpu` — опциональные adapter/device, upload/dispatch/readback и confidence-only hardware measurements;
- `agentir-runtime-cpu` — bounded warmup/timing orchestration над неизменными Stage 8A packages; единственная CPU clock boundary;
- `agentir-store` — atomic file persistence, archive integrity и deterministic replay;
- `agentir-protocol` — wire types и stateful command engine;
- `agentir-cli` — тонкий JSONL stdin/stdout frontend.
- `agentir-policy-eval` — immutable corpus, ranking/search, explicit measurement acquisition, durable recovery/reconciliation, integrated campaigns, replay, metrics, fairness и evaluation archive v8;
- `agentir-eval` CLI — bounded JSONL transport для scripted и внешних agent policies.

Stage 6A quick check:

```bash
cargo run -p agentir-eval -- < examples/eval_free_saxpy.jsonl
cargo run -p agentir-eval -- < examples/eval_compare_policies.jsonl
cargo run --release -p agentir-policy-eval --example evaluation_baseline
```

Stage 6B quick check:

```bash
cargo run -p agentir-eval -- < examples/eval_ranked_hole.jsonl
cargo run -p agentir-eval -- < examples/eval_ranked_compare.jsonl
```

## Ограничения Stage 6B

Evaluation harness измеряет взаимодействие и policy-owned ranking, но не выполняет learned ranking, autotuning, prompt optimization или automatic best-artifact selection. Первый backend по-прежнему ограничен одномерными f32 elementwise kernels. Device execution и hardware measurements являются confidence evidence, а evaluation success означает только выполнение task criterion. Подробности — в [Stage 6B scope](docs/stage-6b-scope.md).

## Независимые hash-контракты

- `content_hash` точно идентифицирует history-sensitive состояние ревизии для replay;
- `spec_hash` идентифицирует семантику complete frozen SpecIR независимо от compiler IDs и истории построения;
- `impl_hash` идентифицирует reachable семантику ImplIR независимо от candidate IDs и provenance;
- `proposal_hash` идентифицирует alpha-normalized proposal до persistent ID allocation;
- `candidate_hash` v1/v2/v3 идентифицирует exact history-sensitive состояние CandidateRevision и его proof state;
- `equality_hash` идентифицирует exact equality state независимо от batching/revision history;
- `memory_hash` идентифицирует exact typed physical plan при неизменном `impl_hash`;
- `target_hash` идентифицирует immutable compiler-owned target capability contract;
- `schedule_hash` идентифицирует exact ScheduleIR state при неизменных `memory_hash` и `target_hash`;
- `backend_hash` идентифицирует typed BackendIR и compiler-owned lowering proof;
- `compiler_build_hash` идентифицирует совместимую версию emitter/validator toolchain;
- `artifact_hash` идентифицирует manifest, ABI и exact ordered WGSL bytes;
- `device_fingerprint_hash` идентифицирует runtime adapter provenance, но не correctness;
- `measurement_hash` идентифицирует completed confidence-only benchmark record;
- `cpu_benchmark_config_hash`, `cpu_input_hash`, `cpu_host_fingerprint_hash` и `cpu_measurement_hash` независимо идентифицируют bounded Stage 8B observation;
- `archive_hash` проверяет конкретный versioned on-disk archive.
- `corpus_hash`, `policy_hash`, `observation_hash`, `episode_hash`, `evaluation_hash` и evaluation `archive_hash` идентифицируют только Stage 6A экспериментальные данные.
- `choice_set_hash`, `feature_schema_hash`, `ranking_policy_hash`, `ranking_trace_hash` и `selection_hash` идентифицируют только Stage 6B ranking data.

Подробный контракт canonical form — в [docs/semantic-canonicalization.md](docs/semantic-canonicalization.md), migration pipeline — в [docs/persistence.md](docs/persistence.md).

## Roadmap

Stage 8 is complete at the offline contract boundary. Only explicit Stage 8B acquisition executes or reads a clock; query/check/archive replay are zero-execution and measurements have no correctness, ranking, or performance-proof authority. See [Stage 8 closure](docs/stage-8-closure.md) and [the roadmap](docs/roadmap.md).

## Документация

Навигация по архитектуре, протоколу, разработке и нормативным исходникам находится в [docs/README.md](docs/README.md).

Проект распространяется по лицензии [MIT](LICENSE).
