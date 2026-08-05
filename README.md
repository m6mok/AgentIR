# AgentIR

AgentIR — экспериментальная агентно-нативная компиляционная среда для численных вычислений. В ней программа хранится не как исходный текст, а как типизированный граф. Агент меняет граф небольшими атомарными ActionIR-транзакциями, а compiler core выводит типы, проверяет формы и сохраняет каждое принятое состояние как неизменяемую ревизию.

Сейчас репозиторий содержит reference prototype Stage 2A. Поверх неизменяемого Stage 1.2 SpecIR он добавляет отдельный ImplIR, persistent CandidateForest, точные compiler-owned rewrites, композиционное доказательство эквивалентности и EvidenceIR. GPU-код по-прежнему не генерируется.

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
- workspace archive v4, явная migration v1 → v2 → v3 → v4, mixed-semantics и candidate replay;
- централизованные resource budgets для core, evaluator, store, protocol и CLI;
- fixed-seed soundness/mutation corpora и statistical benchmark schema v2;
- stateful JSONL CLI с одним ответом на каждый запрос;
- отдельный typed ImplIR и deterministic identity lowering frozen SpecIR;
- immutable candidate revision DAG с fork, atomic rewrite transactions и seal;
- exact known rewrites: unreachable pruning, identical cast elimination и defined scalar constant folding;
- композиционная цепочка `EquivalentToSpec` и разделение correctness/confidence evidence;
- fixed-seed differential validation SpecIR/ImplIR;
- archive/snapshot v4 с explicit v3 → v4 migration и candidate replay.

## Быстрый старт

Нужен stable Rust 1.85 или новее.

```bash
cargo build --workspace
cargo test --workspace
cargo run -p agentir-cli --bin agentir < examples/saxpy.jsonl
cargo run -p agentir-cli --bin agentir < examples/candidate_identity.jsonl
cargo run -p agentir-cli --bin agentir < examples/candidate_rewrite.jsonl
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

- `agentir-core` — SpecIR/ImplIR, verifiers, transactions, revisions, CandidateForest, proofs, EvidenceIR и continuations;
- `agentir-eval` — детерминированный CPU interpreter;
- `agentir-store` — atomic file persistence, archive integrity и deterministic replay;
- `agentir-protocol` — wire types и stateful command engine;
- `agentir-cli` — тонкий JSONL stdin/stdout frontend.

## Ограничения Stage 2A

В прототипе нет speculative rewrites, arbitrary subgraph replacement, approximate refinement, e-graph, candidate ranking/search, performance evidence, MemoryIR, ScheduleIR, TargetManifest, GPU backend или LLVM/MLIR. Shape solver намеренно sound, но incomplete. Workspace можно сохранить в локальный archive и восстановить в новом процессе, но блокировки и многопроцессная координация пока отсутствуют. Известные компромиссы подробно перечислены в [DECISIONS.md](DECISIONS.md).

## Пять разных hash

- `content_hash` точно идентифицирует history-sensitive состояние ревизии для replay;
- `spec_hash` идентифицирует семантику complete frozen SpecIR независимо от compiler IDs и истории построения;
- `impl_hash` идентифицирует reachable семантику ImplIR независимо от candidate IDs и provenance;
- `candidate_hash` идентифицирует exact history-sensitive состояние CandidateRevision;
- `archive_hash` проверяет конкретный versioned on-disk archive.

Подробный контракт canonical form — в [docs/semantic-canonicalization.md](docs/semantic-canonicalization.md), migration pipeline — в [docs/persistence.md](docs/persistence.md).

## Roadmap

Следующий технический шаг — Stage 2B со speculative candidate space и отдельно контролируемым proof debt. Дальнейший путь: MemoryIR → ScheduleIR и simulator → первый GPU backend → обучение и сравнение agent policies. См. [docs/roadmap.md](docs/roadmap.md).

## Документация

Навигация по архитектуре, протоколу, разработке и нормативным исходникам находится в [docs/README.md](docs/README.md).

Проект распространяется по лицензии [MIT](LICENSE).
