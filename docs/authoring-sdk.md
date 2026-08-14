# LLM-native authoring SDK

The local `agentir-authoring` crate is a bounded adapter over the unchanged
production protocol. It accepts exactly one bare model payload in one of three
families, deterministically compiles it to the ordinary
`agentir.elementwise_graph.v1` contract, and uses one shared publication path.
It is not a new canonical language or a production JSONL command.

Choose the smallest surface that expresses the already-determined task:

- `agentir.elementwise_graph.v1` is best for short DAGs where zero-based local
  indices are easy to audit;
- `agentir.elementwise_incremental_batch.v1` is best for longer irregular DAGs:
  small atomic transactions use symbolic `$` bindings and explicit local bases;
- `agentir.elementwise_staged.v1` is best for a fixed bounded repeated shape:
  one body of at most eight operations expands to at most 128 graph operations.

Staged bodies are compiler-owned structural expansion templates, not general
loops and not a source language. All three surfaces preserve exact operand order
and exact `fma` operations.

The exact machine-readable schemas are:

- [graph v1](../schemas/agentir-elementwise-graph-v1.schema.json);
- [transaction v1](../schemas/agentir-elementwise-transaction-v1.schema.json);
- [incremental batch v1](../schemas/agentir-elementwise-incremental-batch-v1.schema.json);
- [staged v1](../schemas/agentir-elementwise-staged-v1.schema.json);
- [compiler-owned frame v2](../schemas/agentir-elementwise-authoring-frame-v2.schema.json);
- [framed staged response v2](../schemas/agentir-elementwise-framed-staged-v2.schema.json).

The evidence, alternatives, design matrix, response binding, and offline
evaluation protocol are recorded in
[authoring ergonomics v2](authoring-ergonomics-v2.md).

`AuthoringSurface::json_schema` and `AuthoringSurface::model_instruction` expose
the corresponding schema and literal short instruction without a second copy
in an integration. `TRANSACTION_JSON_SCHEMA` exposes the nested transaction
contract separately.

The server separately owns:

- task identity, dimension, ordered scalar and tensor declarations;
- runtime inputs;
- the exact ordered intent graph;
- every compiler ID and hash returned by the production engine.

Model payloads cannot represent types, inputs, workspace/revision/artifact IDs,
hashes, guards, certificates, bytecode, source text, or backend settings. The
gateway performs strict parse, local graph construction, task-relative
validation, and exact hidden-intent comparison before `workspace.open`. A
rejected payload therefore consumes no compiler ID and publishes nothing.

An accepted payload becomes one frozen SpecIR through the existing atomic
ActionIR transaction against the compiler-returned base revision. The same
persistent `Engine` session creates the proved candidate, fresh MemoryIR, CPU
target and schedule, emits and verifies portable bytecode, and compares
reference, portable, and optional isolated-native outputs. No pipeline is
copied into the incremental or staged adapters.

Output agreement is execution evidence only. Exact intent acceptance comes
from structural equality with the declared task; neither execution nor the
authoring adapter advances a proof frontier beyond the ordinary compiler-owned
validators.

## Graph surface

~~~json
{
  "schema": "agentir.elementwise_graph.v1",
  "operations": [
    {
      "op": "mul",
      "operands": [
        {"kind": "scalar", "name": "a"},
        {"kind": "tensor", "name": "x"}
      ]
    },
    {
      "op": "add",
      "operands": [
        {"kind": "local", "operation": 0},
        {"kind": "tensor", "name": "y"}
      ]
    }
  ],
  "yield": 1
}
~~~

Local references use a typed integer operation field. There is no textual
v4-to-string conversion. An operation may reference only an earlier index;
failures identify the exact path such as $.operations[2].operands[0].

The literal instruction is exported as both `GRAPH_MODEL_INSTRUCTION` and the
backward-compatible `DEFAULT_MODEL_INSTRUCTION`.

## Incremental batch surface

An incremental payload contains one or more
`agentir.elementwise_transaction.v1` objects and one symbolic final `yield`.
Each transaction contains one to eight operations. Bindings such as `$ax` are
authoring-local, unique for the whole batch, and can be referenced only after
they have been introduced.

`base_operations` is the exact number of operations accepted before the
transaction. For transaction lengths 2, 1, and 3, the bases must be 0, 2, and
3. A duplicate 0, a gap to 3 after the first transaction, or sending the second
transaction first is rejected. This counter is sufficient because the adapter
is a single-session local builder and every accepted transaction is non-empty.
It is not a compiler revision, persistent ID, semantic hash, or concurrency
claim outside that builder session.

Each transaction is applied atomically to a private `IncrementalSession`. A bad
binding, stale base, arity error, or unknown reference leaves the session's
operation count, binding map, and graph unchanged. The complete model payload
still produces only one final graph: if any later transaction or final yield is
bad, the private accepted prefix is dropped before publication.

The checked example is
[authoring_incremental_two_term.json](../examples/authoring_incremental_two_term.json).
`INCREMENTAL_BATCH_MODEL_INSTRUCTION` contains a shorter literal example and
spells out base, binding scope, and yield rules.

## Staged surface

A staged payload repeats one immutable body for a positive fixed stage count.
`stage_local` references an earlier binding in the same body. `state_prev` is
the seed at stage zero and the preceding stage state afterward.
`scalar_cycle` and `tensor_cycle` select a declared capture using
`prefix + ((stage * stride + offset) % count)`; count must be positive. `state`
names the body binding that becomes both the per-stage state and final yield.

`state_lag.initial` is an explicit warmup prefix, not a list whose length must
equal the lag. For lag three and four warmups `[x9, x8, x7, x6]`:

| stage | selected lag operand |
| ---: | --- |
| 0 | `x9` |
| 1 | `x8` |
| 2 | `x7` |
| 3 | `x6` |
| 4 | `state_(4-3) = state_1` |

Only after the entire four-value prefix is exhausted does `state_(i-3)` apply.
Warmup values are scalar/tensor captures, never graph locals. The checked
example is [authoring_staged_two_term.json](../examples/authoring_staged_two_term.json),
and `STAGED_MODEL_INSTRUCTION` includes the four-warmup/lag-three wire form.

## Framed staged v2 surface

Staged v1 remains unchanged. For regular tasks, the server may instead build an
immutable `agentir.elementwise_authoring_frame.v2` from public declarations and
a public recurrence blueprint. The frame fixes stage count, seed, body slots,
named roles, cycle/lag mechanics, state candidates, and the 128-operation cap.
Its deterministic `frame_hash` covers the complete frame.

The model responds with `agentir.elementwise_framed_staged.v2`: exact task and
frame identities, one opcode plus ordered role-ID list per slot, and one final
state slot. It does not repeat stage count, binding names, cycle fields, warmup
arrays, or graph indices. `AuthoringFrame::response_json_schema` returns the
exact task-specific schema, including slot constants and role enums.

`compile_framed_staged` maps roles to the existing staged concepts and expands
to the ordinary graph proposal. `AuthoringGateway::publish_framed_staged`
reconstructs the frame from public declarations, verifies its hash, lowers it,
then uses the existing exact-intent graph publication pipeline. The behavior of
`AuthoringGateway::publish_payload` and all v1 payloads is unchanged.

`recommend_surface` runs before a model call: a short auditable DAG selects
graph, a long irregular DAG selects incremental batch, a valid regular public
blueprint selects framed staged v2, and ambiguous/unsupported inputs return a
typed rejection. It never examines model output or retries another parser.

## Model instructions and trust boundary

Give the model the one surface-specific exported instruction plus one authorized
public task. The names in each literal example show wire shape only. Do not
expose the server task envelope, hidden exact intent, expected outputs,
repository, or unrelated tasks.

Give the model one authorized public task object directly. Do not expose the
server task envelope, exact intent, expected outputs, repository, documentation,
or an array of unrelated tasks in the authoring turn.

Capability and determinacy checks happen before the model call. The server must
not invoke these v1 authoring surfaces when a task needs an opcode outside
`add`/`mul`/`fma`, or when the public task does not prescribe one exact operation
order, exact operand order, and one yield index. A model-authored `div` or another
invented opcode is rejected at its exact path; an alternative commutative order
is still an intent mismatch even if it computes the same mathematical formula.

Diagnostics have a stable code, JSON path, expected value, actual value, and a
surface-specific repair hint. Schema/validation errors may restate the complete
public wire contract. A semantic intent failure reveals only the first local
operation/operand mismatch, or an aggregate operation-count/yield mismatch; it
never returns the complete hidden oracle.

Evaluation runner request v2 carries the exact JSON Schema value, schema
identity/version, no-extra-text and output-size bounds, plus task/session/phase
binding. Repair additionally binds the exact previous raw-payload SHA-256 and
diagnostic code/path, reuses the same session, and remains limited to one
attempt. Provider structured-output capability is recorded separately; it is
never inferred from a schema-valid response.

## CLI

The task file is a server input and exactly one bare model payload arrives on
stdin. The caller may choose the surface explicitly:

~~~bash
cargo run -p agentir-authoring --bin agentir-authoring -- \
  --task examples/authoring_task_two_term.json --surface graph \
  < examples/authoring_proposal_two_term.json

cargo run -p agentir-authoring --bin agentir-authoring -- \
  --task examples/authoring_task_two_term.json --surface incremental-batch \
  < examples/authoring_incremental_two_term.json

cargo run -p agentir-authoring --bin agentir-authoring -- \
  --task examples/authoring_task_two_term.json --surface staged \
  < examples/authoring_staged_two_term.json
~~~

`--surface auto` (and the default when the flag is omitted) dispatches only on
the exact top-level `schema`. It never fallback-parses another dialect after a
failure, so the original diagnostic remains useful.

Success is one JSON object containing the compiler-owned identities, agreed
outputs, and full request/response transcript. Failure is one JSON object with
a stable local error code, path, expected value, and actual value. The caller
must inspect the JSON-level ok field; process exit status is not a replacement.
Schema failures also contain the complete public `repair_hint` contract, so the
caller can return one correction message instead of discovering related shape
errors across several model calls. Both success and failure use one JSON
envelope; JSON-level `ok` is authoritative.

## Scope

This v1 local SDK supports one-dimensional f32 elementwise add, mul, and fma
graphs with at most 128 operations. It changed no production protocol command,
canonical IR, SpecIR/ImplIR/MemoryIR/ScheduleIR/BackendIR rule, archive version,
hash contract, proof frontier, or Stage 9 closure. Promoting an adapter to a
production transport command requires separate scope and an ADR.

## Reasoning canary

A frozen 120-operation residual-mixer prompt was run in fresh isolated
`gpt-5.6-sol` sessions with three `low` and three `medium` reasoning trials. All
six first attempts passed strict schema and matched the independently generated
canonical graph exactly: 120 operations, 276 operands, 36 FMA operations, and
174 local references. The previously recorded `xhigh` trial matched as well.
The canonical result passes atomic publication plus reference and portable CPU
execution.

The two initial low-reasoning chat outputs that were not recorded before their
sessions ended are excluded from this 3/3 denominator, even though they appeared
complete. They were replaced by two fresh low trials whose write-only recording
path was declared before generation. The checked fixtures and oracle comparison
live in `crates/agentir-authoring/tests/high_level_120_agent_trial.rs`.

This canary isolates reasoning level for one deterministic lowering task. It is
not evidence about different model families, ambiguous design tasks, latency,
token cost, or true structured tool calling.

A separate design-choice canary gives three fresh `gpt-5.6-sol low` sessions a
16-stage semantic program with explicit alternatives: FMA versus `mul`+`add`,
recompute versus reuse, and multiple ready-role schedules. All 3/3 first attempts
selected the policy optimum: six operations per stage, 96 total operations, 32
FMA operations, one shared affine result per stage, and the declared
deterministic schedule. An independent lower-bound argument and canonical
constructor grade the choice; every accepted graph also passes reference and
portable CPU execution. The fixtures and grader live in
`crates/agentir-authoring/tests/design_choice_96_agent_trial.rs`.

This still tests selection under an explicit canonical policy. It does not make
execution equality a proof of arbitrary alternative decompositions and does not
add compiler-owned equivalence between different graph shapes.

## Weaker-agent diagnostic ladder

An initial `gpt-5.6-terra low` ladder separates graph reasoning from wire-format
reliability. The first attempts passed an exact five-operation FMA/DAG task and
a 32-operation recurrence, including strict schema, operand order, indices,
publication, and reference/portable execution. On the 96-operation design task,
the model selected the correct minimum design, FMA/reuse policy, cyclic names,
and recovery indices, but consistently emitted `inputs` instead of `operands`
and local `prior` instead of `operation`. Strict parsing rejected the first
operation before publication and consumed no compiler identities.

One path-specific diagnostic plus the complete repair contract corrected all
repeated aliases in one additional model call without changing the design. The
repaired graph matched the independent 96-operation oracle and executed. This
is one trial per ladder level, so it identifies a reproducible failure mode but
does not estimate a stable pass rate. The fixtures and checks live in
`crates/agentir-authoring/tests/weaker_agent_trials.rs` and
`crates/agentir-authoring/tests/design_choice_96_agent_trial.rs`.

### Randomized Terra-low matrix

A follow-up matrix ran twelve blind `gpt-5.6-terra low` trials: four explicit
12-operation DAGs, four 48-operation recurrences, and four 96-operation
design-choice graphs. Six of twelve first attempts matched the strict wire
contract and exact intent. Seven passed the wire schema; the seventh contained
a semantic recovery-lag error. A diagnostic-only normalization of observed
wire aliases showed that nine of twelve had the exact intended graph beneath
format errors.

First-attempt failures were:

- two short graphs with a missing or incorrect top-level schema identifier;
- three recurrence graphs with server-owned declaration fields, of which one
  was otherwise exact, one also used `integer` for local references and changed
  the recurrence at operation 8, and one used string operands and changed the
  recurrence at operation 8;
- one 96-operation graph that used `state_(i-4)` where the task required
  `state_(i-3)`, first diverging at operation 29.

Each of these six failures received one bounded, path-specific repair attempt.
All six repaired proposals then passed strict parsing and exact oracle equality.
The semantic repairs included only the first local expected/actual mismatch and
the original indexing rule; they did not expose the full server intent.

The matrix exposed two harness defects that were excluded from the score. An
initial evaluator prompt incorrectly asked for server-owned declarations, so
those design trials were rerun with the real contract. A later repair was sent
to the wrong task session and produced a schema-valid graph for a different
task; it was discarded and rerun against the originating session. Integrations
must bind repair state to the task and prior proposal identity instead of
accepting an unscoped JSON retry.

The gateway now reports only the first local intent mismatch (or aggregate
length/yield mismatch). It no longer returns the complete server-owned intent
in `expected`, which would turn a repair diagnostic into an oracle leak. The
fixtures, independent constructors, first-attempt taxonomy, and repair checks
live in `crates/agentir-authoring/tests/terra_low_matrix.rs`.

### Luna-medium adversarial matrix

A `gpt-5.6-luna medium` evaluation covered twenty-four tasks from one to 128
operations: exact FMA order, repeated operands, fan-out, non-final yields,
distant references, 99/100/101 boundaries, the exact 128-operation cap,
multiple recurrences, and two 96-operation design graphs.

With eight tasks in a non-production batch envelope and no literal JSON wire
example, three independent sessions invented different plausible dialects:
`type` instead of `kind`; positional `type`/`index` with object-valued yield;
and schema `v1` with symbolic locals such as `l23`. Therefore 0/24 raw
proposals passed strict parsing. Diagnostic-only dialect normalization showed
that all 24/24 matched independent semantic oracles, including both 128-op
cases. One stateless schema repair per batch recovered strict and semantic
equality for all 24/24 proposals.

Three fresh production-shaped single-task controls received the literal wire
example exported by the SDK. All 3/3 passed strict schema on the first attempt.
The 101- and 128-operation recurrences matched intent exactly. The 96-operation
design first diverged at operation 29: Luna used current `state_(i-1)` where the
task required distant `state_(i-3)`, then repeated the lag error. The concrete
example therefore fixes serialization reliability, but compiler-owned
structural recurrence builders or incremental transactions are still needed to
remove manual long-range index arithmetic from weaker models.

The harness exposed additional friction outside graph reasoning: large
generated `apply_patch` payloads commonly failed once on patch framing, and one
excluded run loaded an unrelated local workflow. A production structured call
should accept the JSON value directly, without shell, file editing, repository
instructions, or a batch wrapper. Fixtures, exact constructors, classifier,
and repairs live in `crates/agentir-authoring/tests/luna_medium_edge_matrix.rs`.

### Incremental and staged authoring A/B/C

Two local adapter surfaces remove compiler-index arithmetic while still
lowering to the same ordinary `agentir.elementwise_graph.v1` proposal:

- `agentir.elementwise_incremental_batch.v1` contains
  `agentir.elementwise_transaction.v1` atomic edits of one to eight operations
  against explicit `base_operations`. Results receive symbolic `$` bindings,
  and a stale base or unknown binding rejects only that edit without mutating
  the accepted prefix.
- `agentir.elementwise_staged.v1` repeats a body of at most eight symbolic
  operations. Compiler-owned `state_prev`, `state_lag`, `scalar_cycle`, and
  `tensor_cycle` operands expand deterministically. For `state_lag`, `initial`
  is an explicit warmup prefix whose length can exceed the lag; the recurrence
  starts only after that prefix.

One fresh `gpt-5.6-luna medium` A/B/C trial used the same 96-operation design
task. The raw graph passed schema but first used `state_(i-1)` instead of
`state_(i-3)` at operation 29. The incremental form authored sixteen bounded
six-operation transactions and matched all 96 oracle operations exactly on its
first attempt. The staged form authored only six body operations and selected
the correct lag, cycles, FMA policy, and reuse, but supplied three warmup values
instead of the required four; it therefore diverged earlier at operation 23.
One path-specific repair added `x6` and then matched the complete oracle.

Serialized fixture sizes were 22,139 bytes raw, 13,157 bytes incremental, and
2,371 bytes staged. More importantly, the model authored 96, 96, and 6
operations respectively. This single trial is not a pass-rate estimate, but it
supports the architectural direction: small atomic transactions improve
reference reliability, while structural builders substantially reduce the
reasoning surface. The first staged attempt also exposed and fixed an API flaw:
warmup length must be independent from recurrence lag. Fixtures and executable
checks live in `crates/agentir-authoring/tests/llm_native_interfaces.rs`.
