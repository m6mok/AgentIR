# AgentIR 0.1

## Спецификация агентно-нативного языка и компиляционной среды для высокопроизводительных GPU-вычислений

**Статус:** проект спецификации 0.1  
**Назначение:** формализовать язык, внутренние представления и протокол взаимодействия, в котором программирующими субъектами являются LLM-агенты, а не люди.  
**Рабочее имя:** AgentIR  
**Область версии 0.1:** чистые численные GPU-вычисления над типизированными скалярами, тензорами и буферами.

---

## 0. Нормативные термины

В этом документе слова **ДОЛЖЕН**, **НЕ ДОЛЖЕН**, **СЛЕДУЕТ**, **НЕ СЛЕДУЕТ** и **МОЖЕТ** имеют нормативный смысл.

- **ДОЛЖЕН / НЕ ДОЛЖЕН** — обязательное требование совместимости.
- **СЛЕДУЕТ / НЕ СЛЕДУЕТ** — сильная рекомендация; отклонение требует явного обоснования.
- **МОЖЕТ** — разрешённая, но необязательная возможность.

---

# 1. Назначение системы

AgentIR предназначен для двух совместных целей:

1. сделать построение и оптимизацию программ максимально нативными для LLM;
2. дать компилятору достаточно формальной информации для генерации максимально эффективного кода на целевом GPU.

AgentIR не определяется как текстовый язык. Авторитетной формой программы является типизированная графовая модель с ревизиями, обязательствами доказательства и историей измерений.

Базовая модель взаимодействия:

```text
состояние программы
→ агент выбирает структурированное действие
→ компилятор классифицирует и применяет действие
→ создаётся новая неизменяемая ревизия
→ агент получает только релевантный контекст и допустимые продолжения
```

AgentIR должен позволять нескольким типам агентов работать поверх одной семантики:

- универсальной LLM через JSON/tool calling;
- LLM с constrained decoding;
- специально дообученной модели через компактные opcodes;
- специализированной policy-модели с отдельными выходными головами для opcode, операндов и атрибутов.

---

# 2. Цели и нецели

## 2.1. Цели версии 0.1

Система ДОЛЖНА обеспечивать:

1. функциональную спецификацию вычисления без преждевременной привязки к памяти и железу;
2. типизированное и транзакционное редактирование графа;
3. локальный, инкрементальный цикл проверки;
4. раздельные пространства алгоритма, памяти и расписания исполнения;
5. явную числовую семантику;
6. отсутствие скрытого undefined behavior;
7. формальные или защищённые runtime-проверками предпосылки оптимизаций;
8. ветвящийся поиск нескольких реализаций и расписаний;
9. измерение производительности на целевом оборудовании;
10. воспроизводимое происхождение финального артефакта.

## 2.2. Нецели версии 0.1

В версию 0.1 НЕ входят:

- файловый ввод-вывод;
- сеть и системные вызовы;
- пользовательские интерфейсы;
- операционная система или runtime общего назначения;
- сборщик мусора;
- исключения;
- произвольные указатели;
- произвольный `goto`;
- неограниченная рекурсия;
- общий динамический control flow;
- многопроцессная распределённая оркестрация;
- обучение нейросетей как часть семантики языка;
- автоматическое доверие утверждениям агента;
- обязательная человекочитаемость внутреннего представления.

Оркестрация, загрузка данных, вызов кернелов и управление устройствами выполняются внешним runtime.

---

# 3. Основные аксиомы

## A1. Программа — граф, а не файл

Авторитетное представление программы ДОЛЖНО быть типизированным графом операций, значений, регионов и ограничений. Текст, JSON и бинарная форма являются только кодеками.

## A2. Спецификация отделена от реализации

Оптимизирующий агент НЕ ДОЛЖЕН иметь возможность незаметно изменить задачу ради улучшения benchmark. Спецификация после фиксации неизменяема.

## A3. Верхний слой функционален

В SpecIR входные значения неизменяемы, а операции создают новые логические значения. Физическая мутация появляется только при bufferization в MemoryIR.

## A4. Агент изменяет программу транзакциями

Любое изменение ДОЛЖНО быть атомарной транзакцией относительно известной базовой ревизии.

## A5. Частичная программа допустима

Рабочая ревизия МОЖЕТ содержать типизированные отверстия и незакрытые обязательства. Deployable-ревизия НЕ ДОЛЖНА их содержать.

## A6. Алгоритм, память и schedule разделены

Смысл вычисления, алгоритмическая реализация, физическое хранение и GPU-расписание ДОЛЖНЫ быть отдельными, связанными представлениями.

## A7. Компилятор владеет механической легальностью

Всё, что компилятор может точно и дёшево исключить без потери корректных программ, ДОЛЖНО быть удалено из пространства генерации агента.

## A8. Эвристики не являются запретами

Предполагаемая медленность НЕ ДОЛЖНА делать действие нелегальным. Эвристики влияют на ранжирование, но не на hard constraints.

## A9. Нет скрытого undefined behavior

Предпосылка оптимизации ДОЛЖНА быть:

- доказана;
- защищена runtime guard;
- обеспечена fallback-реализацией;
- либо действие отклоняется.

## A10. Истина о производительности устанавливается измерением

Cost model МОЖЕТ ранжировать кандидатов, но окончательная оценка производительности ДОЛЖНА опираться на измерение на целевом классе устройств, если бюджет это допускает.

## A11. Поиск ветвится

Система НЕ ДОЛЖНА вынуждать агента необратимо выбирать единственную траекторию. Ревизии и кандидаты образуют DAG или дерево.

## A12. Один смысл — одна каноническая форма

На уровне канонической семантики одна операция ДОЛЖНА иметь один opcode и однозначный набор полей. Синонимы и альтернативные синтаксические формы допускаются только во внешних кодеках.

---

# 4. Состав системы

AgentIR состоит из шести логических языков и одной базы состояния.

```text
SpecIR       — что должно быть вычислено
ImplIR       — каким алгоритмом это вычисляется
MemoryIR     — как логические значения материализуются
ScheduleIR   — как работа отображается на GPU
ActionIR     — как агент изменяет остальные представления
EvidenceIR   — доказательства, guards и измерения
Workspace DB — ревизии, кандидаты, holes, цели и target manifests
```

Эти слои МОГУТ использовать одну инфраструктуру операций и регионов, но НЕ ДОЛЖНЫ смешивать семантические роли.

---

# 5. Workspace и состояние программы

## 5.1. Формальная модель

Состояние workspace определяется кортежем:

\[
W = (S, C, T, O, E, H, J, R)
\]

где:

- `S` — замороженная спецификация;
- `C` — лес кандидатов реализации;
- `T` — TargetManifest;
- `O` — открытые ProofObligation;
- `E` — EvidenceIR;
- `H` — типизированные holes;
- `J` — objective и бюджеты;
- `R` — граф ревизий.

## 5.2. Структура workspace

```text
Workspace
├── SpecIR
├── CandidateForest
│   ├── Candidate
│   │   ├── ImplIR
│   │   ├── MemoryIR
│   │   └── ScheduleIR
│   └── ...
├── TargetManifest
├── Objective
├── ProofObligations
├── EvidenceStore
├── BenchmarkStore
└── RevisionGraph
```

## 5.3. Жизненный цикл

Нормативный жизненный цикл:

```text
workspace.open
→ spec.construct
→ spec.check
→ spec.freeze
→ candidate.create
→ transaction.apply
→ candidate.check
→ schedule.explore
→ candidate.benchmark
→ candidate.seal
→ artifact.emit
```

До `spec.freeze` разрешено редактировать SpecIR. `spec.freeze` ДОЛЖЕН требовать установленный набор outputs, отсутствие открытых holes в SpecIR и успешный `SpecComplete` check. После `spec.freeze` оптимизирующие агенты НЕ ДОЛЖНЫ менять SpecIR; изменение спецификации создаёт новый `spec_hash` и новую линию workspace.

---

# 6. Ревизии, идентификаторы и ссылки

## 6.1. Неизменяемость ревизий

Каждая принятая транзакция создаёт новую неизменяемую ревизию.

```text
r17
├── r18: fusion on
├── r19: fusion off
└── r20: alternative reduction
```

## 6.2. Базовая ревизия

Каждая изменяющая транзакция ДОЛЖНА содержать `base_revision`. Если базовая ревизия устарела, сервер ДОЛЖЕН вернуть конфликт, а не молча применить изменение к другому состоянию.

## 6.3. Постоянные идентификаторы

Внутренние постоянные идентификаторы операций, значений, holes и obligations назначаются компилятором. Агент НЕ ДОЛЖЕН создавать глобальные UUID вручную.

## 6.4. Локальные ссылки

Для токеновой эффективности используются:

- `@n` — короткая ссылка на существующий объект текущего контекста;
- `$name` — временная ссылка внутри транзакции;
- `hN` — hole;
- `oN` — proof obligation;
- `cN` — candidate;
- `rN` — revision.

Короткие ссылки действительны только в указанной области или сессии. Авторитетным остаётся внутренний ID.

## 6.5. Хеширование

Система СЛЕДУЕТ хранить отдельно:

```text
spec_hash
implementation_hash
memory_hash
schedule_hash
target_hash
compiler_build_hash
artifact_hash
```

Benchmark record дополнительно ДОЛЖЕН включать fingerprint оборудования, драйвера, распределения входов и протокола измерения.

---

# 7. Общая модель IR

## 7.1. SSA

Значения в SpecIR, ImplIR и большей части MemoryIR ДОЛЖНЫ следовать SSA-модели: операция создаёт новое значение, которое не переназначается.

## 7.2. Операции

Каждая операция содержит:

```text
opcode
operands
result types or type variables
attributes
regions
location/provenance
effects
```

## 7.3. Регионы

Операции высшего порядка (`map`, `reduce`, `scan`) содержат типизированные регионы с block arguments. Регионы имеют явные входы и результаты.

## 7.4. Эффекты

На верхнем уровне допустимы только:

```text
pure
logical_read
logical_write_with_defined_semantics
```

Физические эффекты памяти появляются в MemoryIR и ДОЛЖНЫ быть явными.

---

# 8. Система типов

## 8.1. Скалярные типы 0.1

Обязательное ядро:

```text
bool
i32
i64
f16
bf16
f32
```

Реализация МОЖЕТ дополнительно поддерживать `i8`, `i16`, `u*`, `f64` и target-specific типы, но они не входят в минимальный профиль.

## 8.2. Tensor type

Форма:

```text
tensor<element_type, shape, layout?>
```

Примеры:

```text
tensor<f32, [1024]>
tensor<f16, [M, K]>
tensor<f32, [N, 1]>
```

В SpecIR `layout` обычно отсутствует или логический. Физический layout определяется в MemoryIR.

## 8.3. Shape expressions

Версия 0.1 ДОЛЖНА поддерживать:

- статические целые размеры;
- символические размеры;
- аффинные выражения над символами;
- равенства и неравенства;
- divisibility constraints.

Примеры:

```text
N
2 * N
N + 1
M * K          # неаффинное выражение допускается только как derived size, не как общий solver term
N % 16 == 0
M >= 1
```

Минимальный shape solver ДОЛЖЕН различать:

```text
proved
contradiction
unknown
```

## 8.4. Неявные преобразования

Неявные преобразования типов запрещены. Любая смена числового типа ДОЛЖНА быть операцией `cast` с явным target type и numerical policy.

## 8.5. Broadcasting

Неявный broadcasting запрещён. Используется явная операция `broadcast` или `broadcast_in_dim`.

## 8.6. Типы индексов

Логические индексы используют `index`-семантику. Конкретная ширина выбирается позднее или задаётся ограничением target. Агенту не СЛЕДУЕТ выбирать `i32/i64` для индекса без причины.

---

# 9. Числовая семантика

Числовая семантика НЕ ДОЛЖНА сводиться к одному глобальному `fast_math`.

## 9.1. NumericContract

Контракт может быть глобальным для SpecIR и уточняться для операции:

```json
{
  "storage_type": "f16",
  "compute_type": "f16",
  "accumulator_type": "f32",
  "fma": "allowed",
  "reassociation": false,
  "signed_zero": "preserve",
  "nan": "preserve",
  "infinity": "preserve",
  "denorm": "preserve",
  "determinism": "required",
  "error": {
    "kind": "relative",
    "max": "1e-4"
  }
}
```

## 9.2. Обязательные поля при выборе

Если существуют разные допустимые семантики, агент или спецификация ДОЛЖНЫ выбрать их явно. Это касается:

- FMA contraction;
- reassociation;
- accumulator type;
- deterministic/non-deterministic reduction;
- approximate math intrinsics;
- обработки NaN, infinity, signed zero и denormals;
- допустимой погрешности.

## 9.3. Эквивалентность

Для точного контракта реализация обязана быть семантически эквивалентна SpecIR.

Для приближённого контракта реализация должна удовлетворять отношению refinement с формально или эмпирически проверяемой границей ошибки.

Тестирование повышает confidence, но само по себе НЕ переводит обязательство в `proved`.

---

# 10. SpecIR

## 10.1. Назначение

SpecIR определяет **что** вычисляется, не определяя буферы, GPU threads, tiles и физическое расписание.

SpecIR после `spec.freeze` неизменяем.

## 10.2. Семантическая модель

SpecIR функционален:

```text
outputs = F(inputs, parameters)
```

Входы неизменяемы. Результаты являются новыми логическими значениями.

## 10.3. Обязательные операции 0.1

### Входы и константы

```text
parameter
constant
symbolic_dimension
```

### Скалярная арифметика

```text
add
sub
mul
div
fma
min
max
neg
abs
```

### Логика и выбор

```text
compare
and
or
not
select
```

### Преобразование типов

```text
cast
```

### Тензорные операции

```text
map
zip_map
reduce
scan
broadcast
reshape
transpose
slice
gather
scatter_unique
scatter_reduce
matmul
```

## 10.4. `map`

`map` применяет чистый регион к каждому элементу тензора.

```text
map(input: tensor<T, S>, body: (T) -> U) -> tensor<U, S>
```

## 10.5. `zip_map`

`zip_map` применяет регион к соответствующим элементам тензоров одинаковой формы.

```text
zip_map(inputs: tensor<Ti, S>..., body: (Ti...) -> U) -> tensor<U, S>
```

## 10.6. `reduce`

```text
reduce(
  input,
  axes,
  identity,
  combiner,
  order_semantics,
  determinism
)
```

`order_semantics` принимает минимум:

```text
fixed_order
tree_allowed
```

`tree_allowed` допускается только при совместимом NumericContract.

## 10.7. `scan`

`scan` аналогичен `reduce`, но возвращает промежуточные состояния. Порядок и ассоциативность должны быть явными.

## 10.8. `scatter_unique`

Требует доказательства инъективности индексов либо создаёт `UniqueIndices` obligation.

## 10.9. `scatter_reduce`

Определяет математическое объединение значений при конфликте индексов и тем самым избегает неопределённой гонки.

## 10.10. `matmul`

`matmul` ДОЛЖЕН оставаться высокоуровневой операцией как можно дольше. Его раннее разложение на скалярные циклы НЕ СЛЕДУЕТ выполнять без явной причины.

---

# 11. ImplIR

## 11.1. Назначение

ImplIR представляет выбранный алгоритм, сохраняющий семантику SpecIR.

Примеры решений ImplIR:

- direct vs tiled algorithm;
- tree reduction vs sequential reduction;
- online softmax vs multi-pass softmax;
- fused vs materialized intermediate;
- library call vs custom kernel;
- алгоритмическая аппроксимация в пределах NumericContract.

## 11.2. Отношение к спецификации

Каждый candidate ДОЛЖЕН содержать обязательство:

```text
EquivalentToSpec
```

или:

```text
RefinesSpecWithinTolerance
```

## 11.3. Допустимый control flow

Версия 0.1 допускает:

```text
select
bounded_for
map
reduce
scan
```

`bounded_for` ДОЛЖЕН иметь статическую или доказуемо ограниченную верхнюю границу.

Неограниченные `while`, общая рекурсия и `goto` запрещены.

---

# 12. MemoryIR

## 12.1. Назначение

MemoryIR материализует логические значения в физические регионы памяти.

## 12.2. Буфер

Буфер описывается минимум полями:

```text
element_type
shape
strides
layout
address_space
access
alignment
alias_domain
lifetime
ownership
```

## 12.3. Address spaces

Минимальный GPU-профиль:

```text
global
shared
private
constant
```

Конкретный backend МОЖЕТ расширять этот набор capability-based пространствами.

## 12.4. Нет сырых указателей

В верхнем MemoryIR обращение к памяти задаётся как:

```text
region + typed_index
```

Сырые byte pointers появляются только на позднем lowering и не являются агентным интерфейсом.

## 12.5. In-place reuse

Повторное использование входного буфера для результата является решением bufferization, а не семантикой SpecIR.

Компилятор может выбрать:

```text
proved reuse
runtime-guarded reuse
fresh allocation
fallback temporary
```

## 12.6. Aliasing

Любое `noalias`-подобное свойство должно иметь provenance:

```text
proved_from_type
proved_from_region_construction
proved_from_lifetime
runtime_guard
external_contract
unverified_claim
```

`unverified_claim` НЕ даёт право на опасную оптимизацию.

---

# 13. ScheduleIR

## 13.1. Назначение

ScheduleIR определяет, как алгоритм отображается на аппаратную иерархию.

ScheduleIR не меняет математическую семантику программы.

## 13.2. Минимальные действия

```text
tile
split
fuse
reorder
bind
vectorize
unroll
cache
prefetch
pipeline
tensorize
specialize
materialize
recompute
```

## 13.3. Предусловия

Каждое schedule-действие имеет формальные hard constraints.

Пример:

```text
vectorize width 4
```

может требовать:

```text
stride == 1
alignment >= 16
vector type supported by target
```

Если `extent % 4 == 0` не доказано, компилятор МОЖЕТ создать scalar tail или runtime specialization.

## 13.4. Легальность и полезность

ScheduleIR обязан различать:

- действие физически или семантически незаконно;
- действие законно, но cost model считает его слабым.

Только первое является hard rejection.

## 13.5. Поиск

Schedule search СЛЕДУЕТ представлять иерархическим параметрическим пространством, а не плоским списком всех комбинаций.

---

# 14. TargetManifest

## 14.1. Capability model

AgentIR СЛЕДУЕТ обращаться не к названиям производителей, а к возможностям target:

```text
subgroup_sizes
max_threads_per_workgroup
shared_memory_capacity
register_file_capacity
supported_vector_widths
matrix_instruction_capabilities
async_copy_capabilities
barrier_capabilities
memory_transaction_sizes
cache_hierarchy
address_spaces
atomic_capabilities
numeric_modes
```

## 14.2. Target-specific escape hatch

Версия 0.1 МОЖЕТ поддерживать поздние target-specific intrinsics. Они:

- должны быть перечислены в TargetManifest;
- должны иметь типовую и effect-сигнатуру;
- должны иметь формальную или тестируемую семантику;
- не должны использоваться в SpecIR.

---

# 15. ActionIR

## 15.1. Назначение

ActionIR — каноническая алгебра изменений, доступная агенту.

Агент НЕ редактирует внутренние структуры напрямую.

## 15.2. Классы действий

### Построение

```text
CreateOp
CreateRegion
FillHole
ConnectValue
DefineDimension
AddConstraint
```

### Реализация

```text
SelectAlgorithm
ReplaceSubgraph
ApplyKnownRewrite
ProposeRewrite
MaterializeValue
```

### Память

```text
CreateBufferPlan
ReuseStorage
AddRuntimeGuard
SelectAddressSpace
```

### Schedule

```text
Tile
Split
Fuse
Reorder
Bind
Vectorize
Cache
Prefetch
Pipeline
Tensorize
Specialize
```

### Поиск

```text
ForkCandidate
PruneCandidate
AllocateBudget
MeasureCandidate
CompareCandidates
SealCandidate
```

## 15.3. Транзакция

Нормативная форма:

```json
{
  "workspace": "w1",
  "base_revision": "r17",
  "candidate": "c4",
  "actions": [
    {
      "kind": "create_op",
      "bind": "$p",
      "opcode": "mul",
      "operands": ["@a", "@x"]
    },
    {
      "kind": "create_op",
      "bind": "$r",
      "opcode": "add",
      "operands": ["$p", "@y"]
    },
    {
      "kind": "fill_hole",
      "hole": "@result",
      "value": "$r"
    }
  ]
}
```

## 15.4. Атомарность

Транзакция применяется целиком либо не применяется вообще. При отклонении состояние workspace НЕ ДОЛЖНО меняться.

## 15.5. Вывод типов

Агент НЕ СЛЕДУЕТ заставлять повторять выводимый тип результата. Компилятор возвращает выведенные типы.

Явный тип требуется только при наличии реального выбора, например:

```text
cast target=f16
accumulator=f32
index_width=i64
```

## 15.6. Классификация действия

Каждое действие классифицируется:

```text
legal
conditional
unknown
illegal
```

### `legal`

Доказанно корректно и может применяться немедленно.

### `conditional`

Допустимо при условии. Создаёт proof obligation, guard, specialization или fallback.

### `unknown`

Компилятор не способен установить корректность. Действие может существовать только в speculative candidate и требует дополнительной проверки.

### `illegal`

Нарушает типы, спецификацию, физические ограничения или внутренние инварианты. Не применяется.

---

# 16. Typed holes

## 16.1. Определение

Hole — отсутствующий фрагмент программы с известными требованиями.

```json
{
  "id": "h12",
  "expects": {
    "type": "tensor<f32,[N]>",
    "effects": "pure"
  }
}
```

## 16.2. Использование

Holes позволяют:

- строить программу частями;
- задавать задачи синтеза;
- локализовать контекст;
- выдавать агенту конечное или параметрическое пространство продолжений.

## 16.3. Deployability

Ревизия с незаполненными holes НЕ может быть sealed или deployable.

---

# 17. ContinuationFrame

## 17.1. Назначение

ContinuationFrame описывает пространство допустимого следующего шага. Это центральная сущность agent-native интерфейса.

## 17.2. Структура

```json
{
  "frame": "cf21",
  "revision": "r18",
  "purpose": "fill_hole",
  "focus": {
    "hole": "h7",
    "expects": {
      "type": "tensor<f32,[N]>",
      "effects": "pure"
    }
  },
  "slots": [
    {
      "name": "opcode",
      "kind": "opcode",
      "domain": {
        "enum": ["add", "mul", "fma", "map", "zip_map", "select"]
      }
    },
    {
      "name": "operand_0",
      "kind": "value_ref",
      "depends_on": ["opcode"],
      "domain": {
        "query": "compatible_values",
        "position": 0
      }
    }
  ],
  "escape": {
    "allowed": true,
    "mode": "speculative_proposal",
    "verification_required": true
  }
}
```

## 17.3. Зависимые slots

Домен позднего slot МОЖЕТ зависеть от ранее выбранных значений.

```text
P(action | state)
= P(opcode | state)
× P(arg0 | opcode, state)
× P(arg1 | opcode, arg0, state)
× P(attributes | previous choices, state)
```

## 17.4. Типы доменов

Минимально поддерживаются:

```text
enum
typed reference set
integer interval
symbolic integer set
affine expression domain
constraint system
nested continuation
candidate set
proof method set
```

## 17.5. Hard и soft слои

ContinuationFrame ДОЛЖЕН различать:

```text
hard constraints
soft ranking
```

Пример:

```json
{
  "hard": {
    "tile_size": {
      "min": 32,
      "max": 1024,
      "multiple_of": 32
    }
  },
  "soft": {
    "preferred": [128, 256],
    "reason_code": "OCCUPANCY_ESTIMATE"
  }
}
```

## 17.6. Escape hatch

Меню ДОЛЖНО иметь проверяемый escape hatch там, где потенциально полезна алгоритмическая новизна. Escape-действие создаёт speculative candidate и не получает доверие автоматически.

---

# 18. ProofObligation

## 18.1. Объект обязательства

```json
{
  "id": "o31",
  "kind": "race_freedom",
  "proposition": {
    "forall": ["i", "j"],
    "premise": "i != j",
    "claim": "address(out,i) != address(out,j)"
  },
  "origin": {
    "revision": "r18",
    "action_index": 4
  },
  "status": "open",
  "discharge_methods": [
    "prove_index_map_injective",
    "serialize_axis",
    "replace_with_atomic",
    "introduce_runtime_guard"
  ]
}
```

## 18.2. Основные виды

```text
TypeWellFormed
ShapeCompatible
IndexInBounds
AliasDisjoint
UniqueIndices
RaceFree
DefinedArithmetic
Terminates
EquivalentToSpec
RefinesSpec
ApproximationBound
SynchronizationValid
TargetLegal
ResourceFeasible
Deterministic
```

## 18.3. Состояния

```text
open
proved
guarded
delegated
refuted
unsupported
```

## 18.4. Proof debt

Speculative revision МОЖЕТ иметь открытые обязательства. Система СЛЕДУЕТ ограничивать proof debt бюджетами:

```text
max_open_equivalence_obligations
max_unknown_actions_per_branch
max_speculative_nodes
max_solver_time
```

## 18.5. Seal

Candidate может быть sealed только если все обязательства корректности находятся в состояниях:

```text
proved
guarded
delegated
```

и для каждого `guarded` существует корректная стратегия failure handling.

---

# 19. EvidenceIR

EvidenceIR хранит происхождение и силу фактов.

## 19.1. Виды evidence

```text
formal_proof
solver_result
static_analysis
runtime_guard
translation_validation
differential_test
property_test
hardware_measurement
cost_model_prediction
external_contract
```

## 19.2. Разделение силы свидетельств

Система ДОЛЖНА различать:

```text
correctness evidence
confidence evidence
performance evidence
```

Отсутствие контрпримеров в тестах не равно формальному доказательству.

## 19.3. Provenance

Evidence ДОЛЖНО ссылаться на:

```text
revision
candidate
compiler build
target
input domain
method
parameters
result
```

---

# 20. Objective и бюджеты

## 20.1. Целевая функция

Оптимизация задаётся явной целью:

```json
{
  "objective": {
    "primary": "p99_latency",
    "secondary": ["memory_bytes", "energy"],
    "constraints": {
      "relative_error": "<=1e-4",
      "workspace_memory": "<=64MiB"
    },
    "expected_invocations": 1000000,
    "compile_budget": {
      "wall_time_seconds": 3600,
      "hardware_measurements": 2000,
      "llm_tokens": 500000,
      "candidate_count": 100000
    }
  }
}
```

## 20.2. Стоимость компиляции

СЛЕДУЕТ учитывать ожидаемое число запусков:

\[
J(c) = E[T_{run}(c)] + \frac{T_{compile}(c)}{N_{expected\_runs}}
\]

## 20.3. Многокритериальность

Система МОЖЕТ хранить Pareto archive по:

```text
latency
throughput
energy
memory
code size
numerical error
compile cost
```

---

# 21. Поиск кандидатов

## 21.1. Не один путь

Оптимизатор СЛЕДУЕТ реализовать как сочетание:

```text
LLM policy
compiler-generated legal spaces
analytic models
learned cost model
beam/population search
hardware benchmark
```

## 21.2. Роли

LLM особенно полезна для:

- выбора алгоритма;
- глобальной декомпозиции;
- предложения fusion;
- поиска новых rewrite;
- распределения бюджета;
- выбора направления исследования.

Компилятор владеет:

- типами;
- shapes;
- alias/dependency analysis;
- локальной легальностью;
- lowering;
- instruction selection;
- resource checks.

Cost model ранжирует множество кандидатов, а hardware measurement устанавливает фактическую скорость.

## 21.3. Equality space

Чистые алгебраические альтернативы МОГУТ храниться в e-graph-подобной структуре. NumericContract ограничивает допустимые равенства.

---

# 22. Компиляционный pipeline

Рекомендуемая лестница lowering:

```text
SpecIR
→ ImplIR
→ Algebraic/Fusion space
→ MemoryIR
→ ScheduleIR
→ Tile IR
→ GPU hierarchy IR
→ warp/subgroup IR
→ vector/matrix-instruction IR
→ backend IR
→ machine code
```

Компилятор НЕ СЛЕДУЕТ слишком рано понижать высокоуровневые операции до указателей и циклов, если это уничтожает информацию о тензорах, reductions, layouts или tensor-core semantics.

---

# 23. Временные контуры отзывчивости

## 23.1. Decode-time contour

Во время генерации локально доступны:

- допустимые opcodes;
- живые значения;
- типизированные pointer domains;
- enum domains;
- numeric slot constraints.

Удалённый протокольный вызов после каждого токена запрещён как базовая архитектура.

## 23.2. Transaction contour

После небольшого batch действий выполняются:

```text
schema validation
type checking
shape inference
scope/effect checking
local canonicalization
incremental invalidation
```

## 23.3. Regional compilation contour

Выполняются:

```text
bufferization
fusion legality
schedule legality
resource estimation
partial lowering
candidate code generation
```

## 23.4. Measurement contour

Выполняются:

```text
full backend compilation
autotuning
benchmarking
profile-guided specialization
superoptimization
```

Длительные операции ДОЛЖНЫ иметь task handle, progress, cancellation и воспроизводимый результат.

---

# 24. Инкрементальная query architecture

Анализы ДОЛЖНЫ выражаться запросами:

```text
type_of(value)
shape_of(value)
effects_of(region)
alias_relation(a,b)
race_status(region)
legal_schedules(kernel)
resource_estimate(candidate)
estimated_cost(candidate)
```

Результаты кэшируются по ревизии и dependency graph. После изменения пересчитываются только зависимые запросы.

---

# 25. Протокол и MCP-совместимость

## 25.1. Разделение слоёв

```text
Agent codec / decoder constraints
→ coarse-grained protocol
→ persistent compiler daemon
→ canonical program database
```

MCP МОЖЕТ использоваться как внешний транспорт, но НЕ является семантикой AgentIR.

## 25.2. Рекомендуемые крупные tools

```text
workspace.open
spec.apply
spec.freeze
candidate.create
transaction.apply
program.query
program.check
obligation.resolve
search.start
benchmark.start
candidate.seal
artifact.emit
```

`add`, `mul`, `tile` и другие IR-операции НЕ должны быть отдельными MCP tools. Они передаются как данные внутри `transaction.apply`.

## 25.3. Resources

Большой неизменяемый контекст МОЖЕТ публиковаться как ресурсы:

```text
agentir://workspace/w1/revision/r18
agentir://candidate/c4/graph
agentir://candidate/c4/obligations
agentir://target/t2
agentir://benchmark/b9
agentir://schema/0.1
```

---

# 26. Кодеки

## 26.1. Каноническая семантика

Семантика действий и IR не зависит от внешнего кодека.

## 26.2. Универсальный JSON codec

Предназначен для общей LLM:

```json
{
  "kind": "create_op",
  "opcode": "mul",
  "operands": ["@2", "@5"],
  "bind": "$0"
}
```

## 26.3. Компактный codec

Для дообученного агента:

```text
[c,17,2,5,0]
```

## 26.4. Нативный policy interface

Специализированный агент МОЖЕТ выбирать структуру напрямую:

```text
action_type_head
opcode_head
operand_pointer_head
attribute_head
commit_head
```

## 26.5. Канонические константы

Для точного хеширования floating-point константы СЛЕДУЕТ сериализовать битовым представлением:

```json
{
  "type": "f32",
  "bits": "0x3f800000"
}
```

Пользовательский codec МОЖЕТ принимать десятичную запись и немедленно канонизировать её.

---

# 27. Диагностика

Диагностика ориентирована на агента, а не на человека.

Плохой ответ:

```text
Возможно, x и y пересекаются. Попробуйте noalias.
```

Нормативный структурированный ответ:

```json
{
  "status": "blocked",
  "revision": "r18",
  "code": "ALIAS_UNRESOLVED",
  "obligation": "o7",
  "conflict": ["@x", "@y"],
  "required_fact": {
    "kind": "disjoint_regions",
    "bytes": "N * 4"
  },
  "legal_repairs": [
    {
      "action": "introduce_runtime_guard",
      "template": "regions_disjoint(@x,@y,N*4)"
    },
    {
      "action": "materialize_temporary",
      "source": "@x"
    }
  ],
  "next_frames": ["cf_alias_2"]
}
```

Диагностика ДОЛЖНА по возможности содержать:

```text
stable code
origin
minimal conflicting set
expected property
actual property
repair actions
next continuation
invalidated analyses
```

---

# 28. Состояния кандидата

Минимальная машина состояний:

```text
draft
well_typed
shape_valid
speculative
memory_safe
schedule_legal
lowerable
benchmarkable
sealed
deployable
rejected
```

Переходы должны быть воспроизводимыми и объясняться EvidenceIR.

---

# 29. Безопасность и воспроизводимость

## 29.1. Нет произвольного исполнения

AgentIR server НЕ ДОЛЖЕН исполнять произвольный host-код из атрибутов IR. Intrinsics и внешние вызовы должны быть capability-whitelisted.

## 29.2. Ресурсные бюджеты

Каждая задача должна иметь ограничения:

```text
memory
candidate count
solver time
compile time
benchmark count
artifact size
proof debt
```

## 29.3. Reproducibility manifest

Финальный артефакт ДОЛЖЕН включать или ссылаться на:

```text
spec hash
candidate hashes
target manifest hash
compiler build
runtime guards
proof manifest
benchmark protocol
selected artifact hash
```

---

# 30. Полный пример SAXPY

## 30.1. SpecIR

Математическая задача:

\[
out_i = a \cdot x_i + y_i
\]

Логическая форма:

```text
spec saxpy

input a: f32
input x: tensor<f32,[N]>
input y: tensor<f32,[N]>

output out:
  zip_map(x,y) as (xi,yi):
    fma(a,xi,yi)

numeric:
  fma = allowed
  reassociation = false
  determinism = required
```

## 30.2. Транзакция построения

```json
{
  "workspace": "w1",
  "base_revision": "r0",
  "actions": [
    {
      "kind": "define_dimension",
      "bind": "$N",
      "name": "N",
      "constraints": ["N >= 0"]
    },
    {
      "kind": "create_parameter",
      "bind": "$a",
      "name": "a",
      "type": "f32"
    },
    {
      "kind": "create_parameter",
      "bind": "$x",
      "name": "x",
      "type": "tensor<f32,[$N]>"
    },
    {
      "kind": "create_parameter",
      "bind": "$y",
      "name": "y",
      "type": "tensor<f32,[$N]>"
    },
    {
      "kind": "create_op",
      "bind": "$out",
      "opcode": "zip_map",
      "operands": ["$x", "$y"],
      "region": {
        "arguments": ["xi:f32", "yi:f32"],
        "body": [
          {
            "kind": "create_op",
            "bind": "$v",
            "opcode": "fma",
            "operands": ["$a", "xi", "yi"]
          },
          {
            "kind": "yield",
            "value": "$v"
          }
        ]
      }
    },
    {
      "kind": "set_output",
      "name": "out",
      "value": "$out"
    }
  ]
}
```

## 30.3. Ответ

```json
{
  "status": "accepted",
  "revision": "r1",
  "bindings": {
    "$N": "d1",
    "$a": "v1",
    "$x": "v2",
    "$y": "v3",
    "$out": "v7"
  },
  "inferred": {
    "v7": "tensor<f32,[N]>"
  },
  "obligations_created": [],
  "next_frames": ["cf_freeze_or_extend"]
}
```

## 30.4. Bufferization-кандидаты

```text
A. fresh output buffer
B. reuse y if last_use(y) and snapshot semantics preserved
C. guarded reuse y if x and y do not overlap; otherwise fallback
```

## 30.5. Schedule frame

```json
{
  "frame": "cf_schedule_4",
  "slots": [
    {
      "name": "threads",
      "domain": {
        "min": 32,
        "max": 1024,
        "multiple_of": 32
      },
      "preferred": [128, 256]
    },
    {
      "name": "elements_per_thread",
      "domain": {
        "enum": [1, 2, 4, 8]
      }
    },
    {
      "name": "vector_width",
      "domain": {
        "enum": [1, 2, 4]
      },
      "conditional": {
        "4": ["alignment >= 16"]
      }
    }
  ]
}
```

---

# 31. Минимальный нормативный профиль Stage 1

Stage 1 предназначен не для доказательства превосходства над C/CUDA, а для проверки ключевой гипотезы: LLM эффективнее работает с типизированными транзакциями, holes, ревизиями и continuation frames, чем с генерацией цельного исходного файла.

## 31.1. Обязательный scope

Stage 1 ДОЛЖЕН реализовать:

### Ядро данных

- Workspace;
- immutable Revision DAG;
- SpecIR;
- базовый ActionIR;
- typed holes;
- ProofObligation;
- ContinuationFrame;
- canonical JSON codec;
- reference interpreter на CPU.

### Типы

```text
bool
i32
f32
tensor<T,[static or symbolic dimensions]>
```

### Shape constraints

```text
equality
non-negative symbolic dimensions
same-shape checking
basic affine equality
```

### Операции

```text
parameter
constant
add
sub
mul
div
fma
compare
select
map
zip_map
reduce
cast
```

### API

```text
workspace.open
spec.apply
spec.check
spec.freeze
transaction.apply
program.query
program.evaluate
revision.fork
```

### Поведение

- атомарное применение транзакции;
- вывод типов и shapes;
- отклонение нелегальной операции без изменения состояния;
- генерация continuation frame для hole;
- ветвление ревизий;
- детерминированная каноническая сериализация;
- воспроизводимый hash;
- выполнение SAXPY в reference interpreter.

## 31.2. Явно вне Stage 1

```text
GPU code generation
MLIR/LLVM lowering
MemoryIR implementation
ScheduleIR implementation
autotuning
MCP network server
formal SMT proofs
production security
```

Интерфейсы для этих подсистем МОГУТ быть заглушками, но их реализация не является целью этапа.

## 31.3. Критерий успеха Stage 1

Этап считается успешным, если один и тот же compiler core поддерживает три режима клиента:

1. free structured transaction;
2. compiler-generated continuation choice;
3. hybrid mode с hard constraints и escape proposal;

и позволяет экспериментально измерить:

```text
accepted actions per 1000 tokens
rejected transaction rate
repair cycles
context size
apply latency
semantic correctness
```

---

# 32. План следующих этапов

## Stage 2 — ImplIR и доказательство refinement

- candidate forest;
- known rewrites;
- speculative rewrites;
- reference equivalence checking;
- proof debt;
- e-graph-подобное пространство эквивалентности.

## Stage 3 — MemoryIR

- bufferization;
- regions;
- alias analysis;
- in-place reuse;
- runtime guards;
- reference memory planner.

## Stage 4 — ScheduleIR и CPU/GPU simulator

- tile/split/fuse/bind/vectorize;
- legality engine;
- TargetManifest;
- analytic resource model;
- compiler-generated parametric frames.

## Stage 5 — первый GPU backend

- progressive lowering;
- один target family;
- executable artifacts;
- benchmark harness;
- candidate search.

## Stage 6 — agent training and comparative evaluation

- free/menu/hybrid benchmark suite;
- compact codec;
- policy model;
- cost model;
- hardware feedback loop.

---

# 33. Открытые вопросы после 0.1

1. Нужно ли считать SpecIR частью языка пользователя или отдельным контрактным DSL?
2. Какой класс shape-ограничений должен быть decidable в быстром контуре?
3. Где проходит граница между ImplIR и ScheduleIR для fusion и recomputation?
4. Какие approximate contracts достаточно сильны и при этом практичны?
5. Как представлять target-independent matrix capabilities?
6. Как обучать универсальный action policy без привязки к одному tokenizer?
7. Когда speculative rewrite допускается к hardware benchmark?
8. Как измерять токеновую нативность независимо от конкретной модели?
9. Как стандартизовать proof manifest между backend-ами?
10. Какие части continuation engine должны находиться внутри decoder host?

---

# 34. Краткая формула AgentIR

```text
AgentIR =
  immutable specification
+ typed graph database
+ transactional ActionIR
+ dependent continuation frames
+ explicit proof obligations
+ separate implementation/memory/schedule spaces
+ branching search
+ target-aware lowering
+ measured performance
```

Главный принцип:

> Агент свободно выбирает смысл и направление поиска. Компилятор владеет типами, физической легальностью, доказательствами и материализацией. Производительность подтверждает оборудование.
