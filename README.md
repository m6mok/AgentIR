# AgentIR

AgentIR — экспериментальная агентно-нативная компиляционная среда для численных вычислений. В ней программа хранится не как исходный текст, а как типизированный граф. Агент меняет граф небольшими атомарными ActionIR-транзакциями, а compiler core выводит типы, проверяет формы и сохраняет каждое принятое состояние как неизменяемую ревизию.

Сейчас репозиторий содержит reference prototype Stage 1.1. Он не генерирует GPU-код: задача этапа — проверить архитектуру графа, транзакций, typed holes, semantic canonicalization, воспроизводимых ревизий и эволюционирующих archives.

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
- workspace archive v2, явная migration v1 → v2 и deterministic event replay;
- stateful JSONL CLI с одним ответом на каждый запрос.

## Быстрый старт

Нужен stable Rust 1.85 или новее.

```bash
cargo build --workspace
cargo test --workspace
cargo run -p agentir-cli --bin agentir < examples/saxpy.jsonl
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

- `agentir-core` — canonical IR, verifier, transactions, revisions, holes и continuations;
- `agentir-eval` — детерминированный CPU interpreter;
- `agentir-store` — atomic file persistence, archive integrity и deterministic replay;
- `agentir-protocol` — wire types и stateful command engine;
- `agentir-cli` — тонкий JSONL stdin/stdout frontend.

## Ограничения Stage 1

В прототипе нет GPU backend, LLVM/MLIR, MemoryIR, ScheduleIR, autotuning, SMT solver, сетевого MCP server и concurrent workspace database. Shape solver намеренно мал; unknown shape equality создаёт открытый obligation. Workspace можно сохранить в локальный archive и восстановить в новом процессе, но блокировки и многопроцессная координация пока отсутствуют. Известные компромиссы подробно перечислены в [DECISIONS.md](DECISIONS.md).

## Три разных hash

- `content_hash` точно идентифицирует history-sensitive состояние ревизии для replay;
- `spec_hash` идентифицирует семантику complete frozen SpecIR независимо от compiler IDs и истории построения;
- `archive_hash` проверяет конкретный versioned on-disk archive.

Подробный контракт canonical form — в [docs/semantic-canonicalization.md](docs/semantic-canonicalization.md), migration pipeline — в [docs/persistence.md](docs/persistence.md).

## Roadmap

Следующий технический шаг — ImplIR и проверяемое отношение refinement к замороженному `spec_hash`. Дальнейший путь: MemoryIR → ScheduleIR и simulator → первый GPU backend → обучение и сравнение agent policies. См. [docs/roadmap.md](docs/roadmap.md).

## Документация

Навигация по архитектуре, протоколу, разработке и нормативным исходникам находится в [docs/README.md](docs/README.md).

Проект распространяется по лицензии [MIT](LICENSE).
