# Промт для реализации первого этапа AgentIR

Ниже находится единый промт, рассчитанный на coding-agent. Его можно копировать целиком.

```text
Ты — ведущий инженер компиляторов и системный архитектор. Реализуй первый вертикальный прототип AgentIR 0.1 — агентно-нативной среды программирования, где LLM не пишет цельный исходный файл, а изменяет типизированный граф атомарными транзакциями.

ОСНОВНОЙ КОНТЕКСТ

AgentIR предназначен для будущей генерации высокопроизводительных GPU-кернелов, но первый этап не должен заниматься GPU codegen. Его задача — проверить архитектурную гипотезу:

1. программа хранится как типизированный граф, а не как текстовый файл;
2. агент изменяет её небольшими атомарными транзакциями;
3. компилятор выводит типы и формы, хранит immutable revisions и отклоняет нелегальные изменения без порчи состояния;
4. неполная программа представляется typed holes;
5. компилятор возвращает structured diagnostics и ContinuationFrame — параметрическое пространство допустимого следующего действия;
6. один compiler core поддерживает free, menu и hybrid режимы взаимодействия.

Используй приложенную или доступную в рабочем каталоге спецификацию `agentir_spec_v0_1.md` как источник требований. При противоречии между этим промтом и спецификацией приоритет имеет этот промт для Stage 1, а расхождение нужно зафиксировать в `DECISIONS.md`.

ТЕХНОЛОГИЧЕСКИЙ ВЫБОР

Реализуй проект на стабильном Rust.

Требования:

- workspace Cargo;
- библиотечное ядро без зависимости от сетевого транспорта;
- CLI поверх stdin/stdout в формате JSON Lines;
- `serde` для wire types;
- детерминированные коллекции там, где порядок влияет на сериализацию;
- минимальный и обоснованный набор зависимостей;
- все публичные типы документированы;
- `cargo fmt`, `cargo clippy` и `cargo test` должны проходить;
- unsafe-код не использовать, если без него можно обойтись;
- не подключать MLIR, LLVM, CUDA, ROCm или MCP SDK на первом этапе.

ЦЕЛЬ ЭТАПА

Построй работающий reference prototype, который умеет:

1. открыть workspace;
2. построить и проверить SpecIR;
3. зафиксировать спецификацию;
4. применить транзакцию относительно `base_revision`;
5. создать новую immutable revision;
6. вывести типы и shapes;
7. создать typed hole;
8. выдать ContinuationFrame для hole;
9. ветвить revision DAG;
10. выполнить корректную SpecIR-программу reference interpreter-ом на CPU;
11. детерминированно сериализовать состояние и считать воспроизводимый content hash;
12. полностью построить и выполнить SAXPY.

НЕЦЕЛИ ЭТАПА

Не реализуй:

- GPU code generation;
- ассемблер;
- LLVM или MLIR lowering;
- реальный MemoryIR;
- реальный ScheduleIR;
- autotuning;
- распределённый runtime;
- сетевой MCP server;
- SMT solver;
- сложную оптимизацию;
- пользовательский текстовый синтаксис;
- редактор или UI.

Допускаются только хорошо обозначенные интерфейсы-заглушки для будущих этапов.

АРХИТЕКТУРНЫЕ ИНВАРИАНТЫ

1. Авторитетной программой является типизированный граф.
2. SpecIR функционален: входы неизменяемы, операции создают новые значения.
3. После `spec.freeze` спецификация неизменяема.
4. Каждая принятая транзакция создаёт новую ревизию.
5. Старые ревизии не изменяются.
6. Транзакция атомарна: принимается целиком или не меняет состояние вообще.
7. Клиент обязан передавать `base_revision`.
8. Устаревшая базовая ревизия вызывает structured conflict.
9. Постоянные ID назначает compiler core.
10. Внутри транзакции разрешены временные ссылки `$name`.
11. Для текущего контекста разрешены короткие ссылки `@name` или `@index`, но они должны резолвиться в постоянные ID до commit.
12. Тип результата выводит компилятор. Клиент не дублирует выводимый тип.
13. Неявные cast и broadcasting запрещены.
14. Никакого undefined behavior.
15. Любая ошибка возвращается структурированно и имеет стабильный code.
16. Порядок обхода и сериализации графа детерминирован.

ПРЕДЛАГАЕМАЯ СТРУКТУРА REPOSITORY

Можешь скорректировать структуру, но сохрани разделение ответственности:

agentir/
  Cargo.toml
  README.md
  DECISIONS.md
  crates/
    agentir-core/
      src/
        lib.rs
        ids.rs
        types.rs
        shapes.rs
        ir.rs
        spec.rs
        actions.rs
        transaction.rs
        revision.rs
        workspace.rs
        holes.rs
        continuation.rs
        obligations.rs
        diagnostics.rs
        canonical.rs
        query.rs
    agentir-eval/
      src/
        lib.rs
        value.rs
        interpreter.rs
    agentir-protocol/
      src/
        lib.rs
        request.rs
        response.rs
    agentir-cli/
      src/
        main.rs
  examples/
    saxpy.jsonl
    invalid_type.jsonl
    revision_branch.jsonl
    hole_continuation.jsonl
  tests/
    integration_saxpy.rs
    integration_atomicity.rs
    integration_revisions.rs
    integration_continuation.rs

МИНИМАЛЬНАЯ МОДЕЛЬ ТИПОВ

Реализуй:

- `bool`;
- `i32`;
- `f32`;
- `tensor<T, Shape>`;
- отдельный логический `index`, если это упрощает операции map/reduce;
- symbolic dimensions;
- static dimensions.

Shape должен поддерживать минимум:

- `[4]`;
- `[N]`;
- `[M, N]`;
- равенство символов;
- проверку same-shape;
- constraint `N >= 0`;
- простые аффинные выражения `N + c` и `k * N + c`, если реализация остаётся компактной.

Shape solver возвращает ровно один из статусов:

- `proved`;
- `contradiction`;
- `unknown`.

Не пытайся строить полноценную symbolic algebra system.

МИНИМАЛЬНЫЙ SPECIR

Реализуй operations:

- `parameter`;
- `constant`;
- `add`;
- `sub`;
- `mul`;
- `div`;
- `fma`;
- `compare`;
- `select`;
- `cast`;
- `map`;
- `zip_map`;
- `reduce`.

Все операции должны иметь:

- opcode;
- operand IDs;
- result value IDs;
- attributes;
- optional region;
- source/provenance action ID;
- inferred result type.

Для Stage 1 достаточно одного результата на операцию, но API спроектируй так, чтобы multi-result operations можно было добавить позже без полного слома модели.

REGIONS

`map`, `zip_map` и `reduce` используют region с block arguments и `yield`.

Минимальные требования:

- тип block arguments известен;
- регион не видит произвольные значения, кроме явных captures;
- capture scalar parameter `a` в SAXPY разрешён;
- region verifier проверяет тип yielded value;
- region не имеет side effects.

NUMERIC SEMANTICS STAGE 1

Поддержи минимальный NumericContract:

- `fma`: `forbidden | allowed | required`;
- `reassociation`: bool;
- `determinism`: `required | not_required`.

Reference interpreter должен выполнять строго определённую семантику. Не вводи оптимизации, меняющие порядок вычислений.

ACTIONIR STAGE 1

Поддержи действия:

- `define_dimension`;
- `create_parameter`;
- `create_constant`;
- `create_hole`;
- `create_op`;
- `create_region` или inline region representation;
- `fill_hole`;
- `set_output`;
- `add_constraint`;
- `freeze_spec`;
- `fork_revision`.

Транзакция содержит:

- `workspace`;
- `base_revision`;
- массив `actions`;
- optional client transaction ID.

Временные bindings `$x` действуют только внутри транзакции. После commit сервер возвращает mapping `$x -> persistent_id`.

КЛАССИФИКАЦИЯ ДЕЙСТВИЙ

Каждое действие классифицируется как:

- `legal`;
- `conditional`;
- `unknown`;
- `illegal`.

На Stage 1 реально используй:

- `legal`;
- `illegal`;
- `conditional` для незакрытого shape constraint или hole;
- `unknown` зарезервируй и покрой тестом сериализации.

TYPED HOLES

Hole должен содержать:

- persistent ID;
- expected type;
- expected effects, пока только `pure`;
- optional shape constraints;
- status `open | filled`;
- provenance;
- value, которым он заполнен, если заполнен.

Заполнение hole должно проверять тип и shape.

CONTINUATION FRAME

Сгенерируй ContinuationFrame для выбранного hole.

Минимальный frame должен содержать:

- frame ID;
- revision ID;
- focus hole;
- expected type;
- список допустимых opcode;
- dependent operand slots;
- домен совместимых живых значений;
- hard constraints;
- soft ranking как необязательное поле;
- escape policy.

Не материализуй декартово произведение всех комбинаций. Представляй меню зависимыми slots.

Пример логической формы:

{
  "frame": "cf1",
  "revision": "r4",
  "purpose": "fill_hole",
  "focus": {
    "hole": "h1",
    "expects": "tensor<f32,[N]>"
  },
  "slots": [
    {
      "name": "opcode",
      "domain": ["add", "mul", "fma", "map", "zip_map", "select"]
    },
    {
      "name": "operand_0",
      "depends_on": ["opcode"],
      "domain_query": "compatible_values"
    }
  ],
  "escape": {
    "allowed": true,
    "mode": "speculative_proposal"
  }
}

PROOF OBLIGATIONS STAGE 1

Реализуй объекты obligations минимум для:

- `TypeWellFormed`;
- `ShapeCompatible`;
- `HoleFilled`;
- `SpecComplete`.

Статусы:

- `open`;
- `proved`;
- `refuted`;
- `unsupported`.

Reference prototype не обязан выполнять формальные доказательства beyond type/shape checking.

REVISION DAG

Revision содержит:

- revision ID;
- parent revision IDs;
- content hash;
- root graph IDs;
- applied transaction ID;
- timestamp только как metadata, не участвующий в content hash;
- status summary.

Обязательно реализуй:

- fork от старой ревизии;
- две независимые дочерние ревизии;
- query diff между parent и child;
- запрет мутации parent;
- отказ применять транзакцию к неверной базе, если команда требует current head.

CANONICAL SERIALIZATION И HASH

Нужна детерминированная сериализация, пригодная для content hash.

Требования:

- стабильный порядок полей и элементов;
- metadata вроде времени не участвует в semantic hash;
- floating constants сериализуются точно, предпочтительно как bits, например `f32` + `0x3f800000`;
- один и тот же semantic graph, построенный эквивалентной последовательностью действий, должен иметь одинаковый semantic hash, если это достижимо без чрезмерной сложности;
- если полная независимость от порядка построения не реализована на Stage 1, честно зафиксируй ограничение в `DECISIONS.md` и обеспечь хотя бы стабильность повторной сериализации одной ревизии.

REFERENCE INTERPRETER

Интерпретатор работает на CPU и нужен как oracle семантики.

Поддержи:

- scalar bool/i32/f32;
- dense tensor values;
- parameter binding;
- constants;
- elementwise arithmetic;
- map;
- zip_map;
- reduce;
- select;
- cast;
- output collection.

Обязательно обработай и верни structured errors для:

- отсутствующего input;
- неправильного input type;
- неправильной tensor shape;
- деления на ноль согласно выбранной Stage 1 семантике;
- открытого hole;
- незамороженной или неполной спецификации.

JSONL CLI

CLI читает по одному JSON object на строку и отвечает одним JSON object на строку.

Поддержи команды:

1. `workspace.open`
2. `spec.apply`
3. `spec.check`
4. `spec.freeze`
5. `transaction.apply`
6. `program.query`
7. `program.evaluate`
8. `revision.fork`
9. `revision.diff`
10. `continuation.get`

Команды могут быть реализованы через единый tagged enum.

Все ответы имеют общую оболочку:

{
  "ok": true,
  "request_id": "...",
  "result": { ... },
  "diagnostics": []
}

или:

{
  "ok": false,
  "request_id": "...",
  "error": {
    "code": "TYPE_MISMATCH",
    "message": "краткое машинно-ориентированное описание",
    "origin": { ... },
    "expected": { ... },
    "actual": { ... },
    "repairs": [ ... ]
  }
}

Сообщение может быть читаемым, но клиенты должны иметь возможность работать только по стабильным code и structured fields.

ОБЯЗАТЕЛЬНЫЕ ERROR CODES

Реализуй минимум:

- `WORKSPACE_NOT_FOUND`;
- `REVISION_NOT_FOUND`;
- `BASE_REVISION_CONFLICT`;
- `UNKNOWN_REFERENCE`;
- `DUPLICATE_BINDING`;
- `UNKNOWN_OPCODE`;
- `ARITY_MISMATCH`;
- `TYPE_MISMATCH`;
- `SHAPE_MISMATCH`;
- `INVALID_REGION`;
- `HOLE_TYPE_MISMATCH`;
- `OPEN_HOLE`;
- `SPEC_NOT_COMPLETE`;
- `SPEC_FROZEN`;
- `TRANSACTION_REJECTED`;
- `EVALUATION_INPUT_MISMATCH`.

FREE, MENU И HYBRID РЕЖИМЫ

Один compiler core должен поддерживать три режима клиента:

1. FREE
   Клиент свободно посылает schema-valid ActionIR. Компилятор проверяет после получения транзакции.

2. MENU
   Клиент сначала получает ContinuationFrame и заполняет только предложенные slots. Escape hatch выключен.

3. HYBRID
   Compiler core маскирует доказанно незаконные opcodes/references, возвращает soft ranking и разрешает speculative escape proposal.

На Stage 1 не нужно интегрировать реальную LLM. Реализуй протокольные точки и детерминированный mock client/test harness, чтобы сравнить режимы на одинаковых задачах.

SAXPY — ОБЯЗАТЕЛЬНЫЙ END-TO-END СЦЕНАРИЙ

Построй SpecIR для:

out[i] = a * x[i] + y[i]

Требования:

- `a: f32`;
- `x: tensor<f32,[N]>`;
- `y: tensor<f32,[N]>`;
- `out: tensor<f32,[N]>`;
- реализовать через `zip_map` с capture scalar `a`;
- FMA может быть `allowed`;
- выполнить для `N=4`;
- пример inputs:
  - `a = 2.0`;
  - `x = [1,2,3,4]`;
  - `y = [10,20,30,40]`;
- expected output: `[12,24,36,48]`.

Создай `examples/saxpy.jsonl`, который можно передать CLI и получить этот результат.

ОБЯЗАТЕЛЬНЫЕ ТЕСТЫ

Unit tests:

- scalar type inference;
- tensor type inference;
- same-shape validation;
- cast validation;
- region argument checking;
- hole filling;
- canonical serialization stability;
- content hash stability;
- temporary binding resolution.

Integration tests:

1. SAXPY end-to-end.
2. Неверное сложение `f32 + bool` отклоняется.
3. Неверные shapes `[N]` и `[M]` отклоняются или создают conditional obligation в зависимости от известных constraints.
4. Отклонённая транзакция не меняет revision/head.
5. Две ветки от одной ревизии независимы.
6. Open hole однозначно блокирует `spec.freeze` и `program.evaluate`; ответ содержит `OPEN_HOLE` и список незаполненных holes.
7. ContinuationFrame не предлагает type-incompatible live value.
8. Повторная сериализация даёт одинаковые bytes и hash.
9. Spec после freeze нельзя изменить.
10. CLI JSONL всегда выдаёт один структурированный ответ на один запрос.

BENCHMARKИ ЭТАПА

Добавь microbenchmark или простой измерительный harness для:

- apply транзакции на 1, 10 и 100 операций;
- type/shape query;
- continuation generation;
- canonical serialization;
- reference evaluation SAXPY на нескольких размерах.

Не оптимизируй преждевременно, но зафиксируй baseline и архитектуру, позволяющую позже сделать incremental caching.

README

README должен содержать:

- краткое описание идеи;
- почему это не обычный текстовый язык;
- команды сборки и тестирования;
- команды запуска CLI;
- пример SAXPY;
- описание free/menu/hybrid;
- ограничения Stage 1;
- roadmap до GPU backend.

DECISIONS.md

Зафиксируй:

- принятые архитектурные решения;
- альтернативы;
- почему выбран Rust;
- модель ID;
- модель canonical hash;
- границы shape solver;
- semantics деления на ноль;
- как устроены regions/captures;
- какие части спецификации отложены;
- известные ограничения прототипа.

ПОРЯДОК РАБОТЫ

1. Сначала выдай краткий design summary и дерево проекта.
2. Затем реализуй ядро типов, IDs и revisions.
3. Реализуй IR и verifier.
4. Реализуй transactions и atomic commit.
5. Реализуй holes и continuation frames.
6. Реализуй reference interpreter.
7. Реализуй JSONL protocol и CLI.
8. Добавь examples, tests, README и DECISIONS.
9. Запусти formatter, linter и tests.
10. Исправь все найденные ошибки.
11. В финальном отчёте перечисли созданные файлы, команды запуска, результаты тестов, известные ограничения и точный следующий технический шаг.

КРИТЕРИИ ПРИЁМКИ

Работа принята, если:

- проект собирается одной командой;
- все тесты проходят;
- SAXPY строится только ActionIR-транзакциями;
- SAXPY исполняется reference interpreter-ом и выдаёт `[12,24,36,48]`;
- type-invalid transaction отклоняется атомарно;
- revision branching работает;
- hole получает полезный ContinuationFrame;
- free, menu и hybrid режимы используют один compiler core;
- canonical serialization воспроизводима;
- отсутствует GPU/LLVM/MLIR сложность вне scope;
- README позволяет другому инженеру воспроизвести сценарий без догадок.

Не задавай уточняющих вопросов, если решение можно принять локально. Делай минимальные разумные предположения и записывай их в `DECISIONS.md`. Не подменяй реализацию псевдокодом: нужен работающий, тестируемый repository prototype.
```
