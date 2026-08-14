# Authoring ergonomics v2: framed staged responses

This note records the minimal authoring-only change justified by the frozen
large evaluation under `target/authoring-eval/large-20260813`. It changes no
production protocol command, canonical IR, compiler proof, hash/archive
contract, Stage 9 behavior, or exact-intent acceptance rule.

## Independently verified evidence

The execution plan contains exactly
`30 tasks × 3 surfaces × 3 models × 2 reasoning levels × 1 trial = 540`
initial cells. Events contain 540 unique initial preparations and 216 unique
repair preparations. The saved grades contain 538 determinate initial responses
and 216 repairs. Initial exact-intent success was 322/538; 78/216 repairs
recovered; final success was 400/538.

By surface, final success was graph 153/179 (85.47%), incremental batch 149/179
(83.24%), and staged v1 98/180 (54.44%). Staged strict-schema success was
60/180 (33.33%). The mean staged authored/expanded operation ratio recorded by
the harness was 127,098 millionths, or about 12.71%.

The leading taxonomy counts are combined initial plus repair counts, not
initial-only counts:

| Taxonomy | Initial | Repair | Combined |
| --- | ---: | ---: | ---: |
| `INTENT_OPERAND_MISMATCH` | 72 | 42 | 114 |
| `WRONG_FIELD_TYPE` | 37 | 49 | 86 |
| `BODY_LIMIT` | 68 | 6 | 74 |
| `UNKNOWN_FIELD` | 16 | 22 | 38 |

The leading combined paths were `$.body` (74),
`$.body[0].operands[0].prefix` (40), and
`$.body[0].operands[1].prefix` (26). These are descriptive observations. The
historical experiment did not predeclare a significance test, so no observed
difference is called statistically significant.

## Design-evidence matrix

| Failure family | Concrete saved example | Root cause | Compiler/API fix | Prompt-only fix | Acceptance test |
| --- | --- | --- | --- | --- | --- |
| Wire-format/body expansion | Spark low task 03 staged repair emits 16 stage-specific operations and fails at `$.body` | The model expands stages despite the bounded-body rule | Frame fixes stage count and body slots; v2 response has one choice per slot | Restate “do not unroll” | v2 body 1/8, exact 128 expansion, overflow rejection |
| Wire-format/cycle fields | Spark low task 02 staged initial uses `captures` and later saved grades fail at cycle `prefix` paths | v1 makes the model repeat five mechanical cycle fields and invites plausible dialects | Frame owns cycle mechanics; the model names one role ID | Add a literal cycle example | unknown role/type/field tests and exact scalar/tensor cycle expansion |
| Local lowering/reference | Saved staged repairs use `state_local`/`state` instead of `stage_local`; graph/incremental failures contain wrong distant indices | Models perform mechanical addressing and family selection | Named frame roles lower to v1 `stage_local`, `state_prev`, `state_lag`, and cycles | Explain every reference family | fixed-seed independent expansion, lag warmup, fan-out/distant reuse coverage |
| Exact semantic intent | Spark low task 02 incremental initial first diverges at operation 8 operand 0 (`signal_coef1` vs `signal_coef7`) | The payload is valid but selects the wrong public semantic role | Keep first-local exact-intent diagnostic and unchanged gateway equality | Remind the model to recheck the governing public rule | exact operand order and FMA-boundary rejection |
| Provider/framing | Ten saved non-JSON grades, three extra-text grades, and two indeterminate initial provider outcomes | Text-only transport and uncertain provider completion | Runner request v2 embeds the exact JSON Schema, size bound, binding, and structured-output policy; raw bytes still precede grading | “Return one JSON object” | request-contract tests, response size test, read-only historical replay |
| Repair identity | Earlier operator report records a repair sent to the wrong task session | Repair was scoped by convention rather than a machine identity | Bind task, logical session, surface/schema, prior raw SHA-256, diagnostic code/path, phase, and one repair attempt | Repeat the task ID in prose | mismatched session/phase test and payload-hash binding test |
| Harness/operator | Earlier evaluator prompt requested server-owned declarations; a shell glob overcounted; replay was accidentally invoked twice | Integration/operator mistakes outside model semantics | Prompt-oracle audit, exact plan cells, durable prepare markers, and read-only `verify-replay` | Operator checklist | audit tests, unique event counts, historical 754-grade verification |

The matrix distinguishes wire failures, local lowering/reference failures,
semantic intent failures, provider failures, harness failures, and prior operator
mistakes. A schema-valid response is not considered semantically correct.

## Alternatives considered

1. Named capture groups reduce repeated capture lists but still require the
   model to choose reference families and author cycle/lag parameters. They do
   not directly prevent body unrolling.
2. A compiler-generated operand-role menu/frame removes all mechanical stage,
   cycle, warmup, binding, and graph-index fields from the model response while
   preserving explicit opcode, operand-role, and final-state decisions. The
   task-specific response schema is small, strict, and directly testable.
3. Several specialized staged schemas can make each recurrence concise, but
   multiply schema versions and parser branches, make mixed regular bodies
   awkward, and increase compatibility/testing cost.

Option 2 is selected. It has the fewest model-authored mechanical degrees of
freedom without encoding a new canonical language. Named roles are used inside
the selected frame; specialized schema families are not added.

## Contracts and lowering

`agentir.elementwise_staged.v1` remains byte-for-byte and behaviorally intact.
The new server object is `agentir.elementwise_authoring_frame.v2`; the model
response is `agentir.elementwise_framed_staged.v2`.

The frame is built only from `PublicAuthoringDeclarations` and an explicit
public blueprint. That type contains task ID and capture declarations but no
runtime inputs or hidden intent. It fixes stage count, seed role, one to eight
body slots, named operand roles, allowed opcode/operand menus, state candidates,
and the 128-operation cap. Its SHA-256 `frame_hash` covers every one of those
fields. The gateway reconstructs and verifies this identity before lowering or
opening a workspace.

The response copies only `task_id`, `frame_hash`, one `{slot, op, operands}`
choice per body slot, and `state`. It never authors stages, seed, bind names,
`prefix/count/stride/offset`, warmup arrays, or graph indices. The compiler maps
roles to the existing staged v1 concepts, expands deterministically to an
ordinary `agentir.elementwise_graph.v1`, and calls the unchanged graph gateway.
Exact hidden graph equality still precedes `workspace.open`; execution equality
does not substitute for it.

Warmup semantics are unchanged: `state_lag.initial` is a complete prefix. Its
length may equal or exceed the positive lag. A shorter prefix is rejected
because `state_(i-lag)` would not yet exist when the prefix ends.

## Pre-call surface recommendation

Recommendation consumes only public problem shape:

- an exact short auditable DAG of at most 16 operations uses graph v1;
- a longer or non-auditable exact DAG of at most 128 uses incremental batch v1;
- an exact regular recurrence whose blueprint passes frame validation uses
  framed staged v2;
- ambiguous public intent, zero operations, overflow, undeclared captures, or
  invalid recurrence mechanics produce a typed capability rejection before a
  model call.

The recommendation never observes model output and never fallback-parses a
different dialect.

## Runner v2 and repair binding

`agentir.authoring_eval.runner_request.v2` embeds a versioned response contract:
the exact selected JSON Schema value, schema ID/version, required top-level
schema, no-extra-text rule, 262,144-byte maximum, structured-output policy, and
task/session/phase binding. A runner must echo the exact session and phase.
Structured-output support/enforcement is transport metadata; missing evidence
is recorded as `unreported` and is never presented as enforcement.

A repair reuses the exact logical session and adds the SHA-256 of the verbatim
previous raw payload, diagnostic taxonomy/path, surface/schema, and
`repair_attempt=1`. Schema diagnostics may return the public contract. Semantic
diagnostics still reveal only one local mismatch or aggregate length/yield and
ask the model to recheck every reference governed by the same public rule. They
never return the complete hidden graph.

## Offline evaluation

Generate the immutable v2 corpus, prompts, task-specific schemas, oracle audit,
and paired plan without provider calls:

```bash
AGENTIR_AUTHORING_EVAL_MODELS='gpt-5.6-terra,gpt-5.6-luna,gpt-5.3-codex-spark' \
AGENTIR_AUTHORING_EVAL_REASONING_LEVELS='low,medium' \
cargo run -p agentir-authoring --bin agentir-authoring-eval -- \
  generate-v2 --output target/authoring-eval/ergonomics-v2-20260814
```

The predeclared comparison is staged v1 versus framed staged v2 over
`30 × 2 × 3 × 2 × 1 = 360` initial calls, at most 360 conditional repairs, and
720 total calls. The primary metric is paired initial exact-intent success. The
only inferential analysis is an exact two-sided McNemar test at alpha 0.05 over
task/model/reasoning/trial pairs. Secondary metrics are descriptive.

No run/provider command exists for this v2 plan while
`PAID_CALLS_AUTHORIZED=false`. A separate user authorization must name the exact
call count before external execution is implemented or performed.
