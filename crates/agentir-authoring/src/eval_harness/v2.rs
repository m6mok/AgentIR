use super::corpus::{audit_prompts, build_corpus, prompt_for};
use super::{
    AnyResult, Configuration, FORMAT_VERSION, PrivateCorpusTask, PublicCorpusTask, SurfaceName,
    atomic_json, atomic_write,
};
use agentir_authoring::{
    AuthoringFrame, AuthoringFrameBlueprint, AuthoringTask, FRAMED_STAGED_MODEL_INSTRUCTION,
    FRAMED_STAGED_SCHEMA, FrameOpcodeMenu, FrameRole, FrameSlot, FramedOperationChoice,
    FramedStagedProposal, GraphOpcode, GraphOperand, PublicAuthoringDeclarations, STAGED_SCHEMA,
    StagedOperand, StagedProposal, build_authoring_frame, compile_framed_staged,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

const V2_FORMAT_VERSION: u32 = 2;
const V2_TRIALS: usize = 1;
const V2_SURFACES: [V2Surface; 2] = [V2Surface::StagedV1, V2Surface::FramedStagedV2];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum V2Surface {
    StagedV1,
    FramedStagedV2,
}

impl V2Surface {
    const fn name(self) -> &'static str {
        match self {
            Self::StagedV1 => "staged_v1",
            Self::FramedStagedV2 => "framed_staged_v2",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ErgonomicsMetrics {
    staged_v1_model_authored_fields: usize,
    framed_v2_model_authored_fields: usize,
    field_reduction_micros: u64,
    staged_v1_model_authored_operations: usize,
    framed_v2_model_authored_operations: usize,
    expanded_operations: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2PublicTask {
    task: PublicCorpusTask,
    frame: AuthoringFrame,
    response_json_schema: Value,
    ergonomics: ErgonomicsMetrics,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2PrivateTask {
    public: V2PublicTask,
    server_task: AuthoringTask,
    staged_v1_oracle: StagedProposal,
    framed_v2_oracle: FramedStagedProposal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2PublicCorpus {
    format: String,
    format_version: u32,
    source_corpus_format_version: u32,
    corpus_hash: String,
    tasks: Vec<V2PublicTask>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2PrivateCorpus {
    format: String,
    format_version: u32,
    source_corpus_format_version: u32,
    corpus_hash: String,
    tasks: Vec<V2PrivateTask>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2Cell {
    cell_id: String,
    task_id: String,
    model: String,
    reasoning_level: String,
    surface: V2Surface,
    trial_index: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2ExecutionPlan {
    format: String,
    format_version: u32,
    corpus_hash: String,
    formula: String,
    tasks: usize,
    surfaces: usize,
    models: usize,
    reasoning_levels: usize,
    trials: usize,
    planned_initial_calls: usize,
    planned_conditional_repair_calls: usize,
    planned_maximum_total_calls: usize,
    maximum_repairs_per_initial: usize,
    paid_calls_authorized: bool,
    primary_metric: String,
    statistical_test: String,
    cells: Vec<V2Cell>,
}

pub(crate) fn generate_v2(output: &Path, configuration: &Configuration) -> AnyResult<()> {
    std::fs::create_dir_all(output)?;
    let source = build_corpus()?;
    audit_prompts(&source)?;
    let tasks = source
        .iter()
        .map(build_v2_task)
        .collect::<AnyResult<Vec<_>>>()?;
    audit_v2_prompts(&tasks)?;
    let corpus_hash = v2_corpus_hash(&tasks)?;
    let public = V2PublicCorpus {
        format: "agentir.authoring_eval.ergonomics_v2.corpus.public".to_owned(),
        format_version: V2_FORMAT_VERSION,
        source_corpus_format_version: FORMAT_VERSION,
        corpus_hash: corpus_hash.clone(),
        tasks: tasks.iter().map(|task| task.public.clone()).collect(),
    };
    let private = V2PrivateCorpus {
        format: "agentir.authoring_eval.ergonomics_v2.corpus.private".to_owned(),
        format_version: V2_FORMAT_VERSION,
        source_corpus_format_version: FORMAT_VERSION,
        corpus_hash: corpus_hash.clone(),
        tasks: tasks.clone(),
    };
    let plan = build_v2_plan(configuration, &corpus_hash, &tasks)?;
    atomic_json(&output.join("corpus.json"), &public)?;
    atomic_json(&output.join("corpus-private.json"), &private)?;
    atomic_write(
        &output.join("corpus-hash.txt"),
        format!("{corpus_hash}\n").as_bytes(),
    )?;
    atomic_json(&output.join("execution-plan.json"), &plan)?;
    write_v2_prompts_and_contracts(output, &tasks)?;
    atomic_json(
        &output.join("prompt-oracle-audit.json"),
        &json!({
            "format":"agentir.authoring_eval.prompt_oracle_audit.v2",
            "passed":true,
            "tasks":tasks.len(),
            "surfaces":V2_SURFACES.len(),
            "checks":[
                "no server runtime inputs",
                "no hidden ordinary graph payload",
                "no private staged or framed oracle response",
                "frame contains public addressing facts and no selected opcode/operand choices",
                "repair contract permits one local diagnostic only"
            ]
        }),
    )?;
    atomic_json(
        &output.join("experiment.json"),
        &json!({
            "format":"agentir.authoring_eval.ergonomics_v2.experiment",
            "format_version":V2_FORMAT_VERSION,
            "corpus_hash":corpus_hash,
            "models":configuration.models,
            "reasoning_levels":configuration.reasoning_levels,
            "paid_calls_authorized":false,
            "external_calls_executed":false,
            "structured_output_provider_evidence":false,
            "historical_artifacts_mutated":false,
        }),
    )?;
    write_design(output, &plan, &tasks)?;
    println!(
        "generated ergonomics v2 offline plan: {} × {} × {} × {} × {} = {} initial; {} repair; {} maximum; zero model calls",
        plan.tasks,
        plan.surfaces,
        plan.models,
        plan.reasoning_levels,
        plan.trials,
        plan.planned_initial_calls,
        plan.planned_conditional_repair_calls,
        plan.planned_maximum_total_calls
    );
    Ok(())
}

fn build_v2_task(source: &PrivateCorpusTask) -> AnyResult<V2PrivateTask> {
    let (blueprint, framed_oracle) = frame_from_public_recipe(source)?;
    let declarations = PublicAuthoringDeclarations::from(&source.server_task);
    let frame = build_authoring_frame(&declarations, &blueprint)?;
    let mut framed_oracle = framed_oracle;
    framed_oracle.task_id.clone_from(&frame.task_id);
    framed_oracle.frame_hash.clone_from(&frame.frame_hash);
    let lowered = compile_framed_staged(&frame, &framed_oracle)?;
    if lowered != source.server_task.intent {
        return Err(format!("framed v2 oracle disagrees for {}", source.public.task_id).into());
    }
    let v1_fields = authored_fields(&serde_json::to_value(&source.staged_payload)?);
    let v2_fields = authored_fields(&serde_json::to_value(&framed_oracle)?);
    let reduction = if v1_fields == 0 {
        0
    } else {
        u64::try_from((v1_fields.saturating_sub(v2_fields) as u128 * 1_000_000) / v1_fields as u128)
            .unwrap_or(u64::MAX)
    };
    Ok(V2PrivateTask {
        public: V2PublicTask {
            task: source.public.clone(),
            response_json_schema: frame.response_json_schema(),
            frame,
            ergonomics: ErgonomicsMetrics {
                staged_v1_model_authored_fields: v1_fields,
                framed_v2_model_authored_fields: v2_fields,
                field_reduction_micros: reduction,
                staged_v1_model_authored_operations: source.staged_payload.body.len(),
                framed_v2_model_authored_operations: framed_oracle.choices.len(),
                expanded_operations: lowered.operations.len(),
            },
        },
        server_task: source.server_task.clone(),
        staged_v1_oracle: source.staged_payload.clone(),
        framed_v2_oracle: framed_oracle,
    })
}

fn frame_from_public_recipe(
    source: &PrivateCorpusTask,
) -> AnyResult<(AuthoringFrameBlueprint, FramedStagedProposal)> {
    let staged = &source.staged_payload;
    let mut roles = BTreeMap::new();
    let mut role_by_value = BTreeMap::new();
    let seed_role = insert_direct_role(&mut roles, &mut role_by_value, &staged.seed)?;
    for scalar in &source.public.scalars {
        insert_named_role(
            &mut roles,
            &mut role_by_value,
            format!("scalar_{scalar}"),
            FrameRole::Scalar {
                name: scalar.clone(),
            },
        )?;
    }
    for tensor in &source.public.tensors {
        insert_named_role(
            &mut roles,
            &mut role_by_value,
            format!("tensor_{tensor}"),
            FrameRole::Tensor {
                name: tensor.clone(),
            },
        )?;
    }
    for (slot_index, operation) in staged.body.iter().enumerate() {
        if slot_index > 0 {
            let prior_slot = staged.body[slot_index - 1]
                .bind
                .trim_start_matches('$')
                .to_owned();
            insert_named_role(
                &mut roles,
                &mut role_by_value,
                format!("local_{prior_slot}"),
                FrameRole::StageLocal { slot: prior_slot },
            )?;
        }
        for operand in &operation.operands {
            insert_staged_role(&mut roles, &mut role_by_value, operand)?;
        }
    }
    let mut slots = Vec::new();
    let mut choices = Vec::new();
    for (slot_index, operation) in staged.body.iter().enumerate() {
        let slot_id = operation.bind.trim_start_matches('$').to_owned();
        let allowed = roles
            .iter()
            .filter(|(_, role)| match role {
                FrameRole::StageLocal { slot } => staged
                    .body
                    .iter()
                    .position(|item| item.bind.trim_start_matches('$') == slot)
                    .is_some_and(|index| index < slot_index),
                _ => true,
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let menus = [GraphOpcode::Add, GraphOpcode::Mul, GraphOpcode::Fma]
            .into_iter()
            .map(|op| FrameOpcodeMenu {
                op,
                operand_roles: vec![allowed.clone(); opcode_arity(op)],
            })
            .collect();
        let operands = operation
            .operands
            .iter()
            .map(|operand| {
                let key = serde_json::to_string(operand)?;
                role_by_value
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| "public operand role was not materialized".into())
            })
            .collect::<AnyResult<Vec<_>>>()?;
        slots.push(FrameSlot {
            id: slot_id.clone(),
            menus,
        });
        choices.push(FramedOperationChoice {
            slot: slot_id,
            op: operation.op,
            operands,
        });
    }
    let state = staged.state.trim_start_matches('$').to_owned();
    Ok((
        AuthoringFrameBlueprint {
            stages: staged.stages,
            seed_role,
            roles,
            slots,
            state_candidates: staged
                .body
                .iter()
                .map(|operation| operation.bind.trim_start_matches('$').to_owned())
                .collect(),
        },
        FramedStagedProposal {
            schema: FRAMED_STAGED_SCHEMA.to_owned(),
            task_id: String::new(),
            frame_hash: String::new(),
            choices,
            state,
        },
    ))
}

fn insert_direct_role(
    roles: &mut BTreeMap<String, FrameRole>,
    values: &mut BTreeMap<String, String>,
    operand: &GraphOperand,
) -> AnyResult<String> {
    let (id, role) = match operand {
        GraphOperand::Scalar { name } => (
            format!("scalar_{name}"),
            FrameRole::Scalar { name: name.clone() },
        ),
        GraphOperand::Tensor { name } => (
            format!("tensor_{name}"),
            FrameRole::Tensor { name: name.clone() },
        ),
        GraphOperand::Local { .. } => return Err("public frame seed cannot be local".into()),
    };
    insert_named_role(roles, values, id.clone(), role)?;
    Ok(id)
}

fn insert_staged_role(
    roles: &mut BTreeMap<String, FrameRole>,
    values: &mut BTreeMap<String, String>,
    operand: &StagedOperand,
) -> AnyResult<String> {
    let key = serde_json::to_string(operand)?;
    if let Some(existing) = values.get(&key) {
        return Ok(existing.clone());
    }
    let (base, role) = match operand {
        StagedOperand::Scalar { name } => (
            format!("scalar_{name}"),
            FrameRole::Scalar { name: name.clone() },
        ),
        StagedOperand::Tensor { name } => (
            format!("tensor_{name}"),
            FrameRole::Tensor { name: name.clone() },
        ),
        StagedOperand::StageLocal { name } => {
            let slot = name.trim_start_matches('$').to_owned();
            (format!("local_{slot}"), FrameRole::StageLocal { slot })
        }
        StagedOperand::StatePrev => ("state_prev".to_owned(), FrameRole::StatePrev),
        StagedOperand::StateLag { stages, initial } => (
            "state_lag".to_owned(),
            FrameRole::StateLag {
                stages: *stages,
                initial: initial.clone(),
            },
        ),
        StagedOperand::ScalarCycle {
            prefix,
            count,
            stride,
            offset,
        } => (
            "scalar_cycle".to_owned(),
            FrameRole::ScalarCycle {
                prefix: prefix.clone(),
                count: *count,
                stride: *stride,
                offset: *offset,
            },
        ),
        StagedOperand::TensorCycle {
            prefix,
            count,
            stride,
            offset,
        } => (
            "tensor_cycle".to_owned(),
            FrameRole::TensorCycle {
                prefix: prefix.clone(),
                count: *count,
                stride: *stride,
                offset: *offset,
            },
        ),
    };
    let id = unique_role_id(roles, base);
    roles.insert(id.clone(), role);
    values.insert(key, id.clone());
    Ok(id)
}

fn insert_named_role(
    roles: &mut BTreeMap<String, FrameRole>,
    values: &mut BTreeMap<String, String>,
    id: String,
    role: FrameRole,
) -> AnyResult<()> {
    let key = role_key(&role)?;
    if let Some(existing) = values.get(&key) {
        if roles.get(existing) != Some(&role) {
            return Err("role identity collision".into());
        }
        return Ok(());
    }
    let id = unique_role_id(roles, id);
    roles.insert(id.clone(), role);
    values.insert(key, id);
    Ok(())
}

fn role_key(role: &FrameRole) -> AnyResult<String> {
    let staged = match role {
        FrameRole::Scalar { name } => StagedOperand::Scalar { name: name.clone() },
        FrameRole::Tensor { name } => StagedOperand::Tensor { name: name.clone() },
        FrameRole::StageLocal { slot } => StagedOperand::StageLocal {
            name: format!("${slot}"),
        },
        FrameRole::StatePrev => StagedOperand::StatePrev,
        FrameRole::StateLag { stages, initial } => StagedOperand::StateLag {
            stages: *stages,
            initial: initial.clone(),
        },
        FrameRole::ScalarCycle {
            prefix,
            count,
            stride,
            offset,
        } => StagedOperand::ScalarCycle {
            prefix: prefix.clone(),
            count: *count,
            stride: *stride,
            offset: *offset,
        },
        FrameRole::TensorCycle {
            prefix,
            count,
            stride,
            offset,
        } => StagedOperand::TensorCycle {
            prefix: prefix.clone(),
            count: *count,
            stride: *stride,
            offset: *offset,
        },
    };
    Ok(serde_json::to_string(&staged)?)
}

fn unique_role_id(roles: &BTreeMap<String, FrameRole>, base: String) -> String {
    if !roles.contains_key(&base) {
        return base;
    }
    let mut suffix = 2_usize;
    loop {
        let candidate = format!("{base}_{suffix}");
        if !roles.contains_key(&candidate) {
            return candidate;
        }
        suffix = suffix.checked_add(1).expect("role suffix space exhausted");
    }
}

const fn opcode_arity(opcode: GraphOpcode) -> usize {
    match opcode {
        GraphOpcode::Add | GraphOpcode::Mul => 2,
        GraphOpcode::Fma => 3,
    }
}

fn authored_fields(value: &Value) -> usize {
    match value {
        Value::Object(object) => object.len() + object.values().map(authored_fields).sum::<usize>(),
        Value::Array(values) => values.iter().map(authored_fields).sum(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => 0,
    }
}

fn build_v2_plan(
    configuration: &Configuration,
    corpus_hash: &str,
    tasks: &[V2PrivateTask],
) -> AnyResult<V2ExecutionPlan> {
    let initial = tasks
        .len()
        .checked_mul(V2_SURFACES.len())
        .and_then(|count| count.checked_mul(configuration.models.len()))
        .and_then(|count| count.checked_mul(configuration.reasoning_levels.len()))
        .and_then(|count| count.checked_mul(V2_TRIALS))
        .ok_or("v2 plan size overflow")?;
    let mut cells = Vec::with_capacity(initial);
    for model in &configuration.models {
        for reasoning in &configuration.reasoning_levels {
            for task in tasks {
                for surface in V2_SURFACES {
                    for trial_index in 0..V2_TRIALS {
                        cells.push(V2Cell {
                            cell_id: format!(
                                "{}__{}__{}__{}__{}",
                                super::safe_component(model),
                                super::safe_component(reasoning),
                                task.public.task.task_id,
                                surface.name(),
                                trial_index
                            ),
                            task_id: task.public.task.task_id.clone(),
                            model: model.clone(),
                            reasoning_level: reasoning.clone(),
                            surface,
                            trial_index,
                        });
                    }
                }
            }
        }
    }
    Ok(V2ExecutionPlan {
        format: "agentir.authoring_eval.ergonomics_v2.execution_plan".to_owned(),
        format_version: V2_FORMAT_VERSION,
        corpus_hash: corpus_hash.to_owned(),
        formula: format!(
            "{} tasks × {} surfaces × {} models × {} reasoning levels × {} trial",
            tasks.len(),
            V2_SURFACES.len(),
            configuration.models.len(),
            configuration.reasoning_levels.len(),
            V2_TRIALS
        ),
        tasks: tasks.len(),
        surfaces: V2_SURFACES.len(),
        models: configuration.models.len(),
        reasoning_levels: configuration.reasoning_levels.len(),
        trials: V2_TRIALS,
        planned_initial_calls: initial,
        planned_conditional_repair_calls: initial,
        planned_maximum_total_calls: initial * 2,
        maximum_repairs_per_initial: 1,
        paid_calls_authorized: false,
        primary_metric: "initial_exact_intent_success".to_owned(),
        statistical_test: "paired exact two-sided McNemar test, alpha=0.05, discordant pairs keyed by task/model/reasoning/trial".to_owned(),
        cells,
    })
}

fn prompt_v2(task: &V2PrivateTask, surface: V2Surface) -> AnyResult<String> {
    Ok(match surface {
        V2Surface::StagedV1 => prompt_for_source(task, SurfaceName::Staged),
        V2Surface::FramedStagedV2 => format!(
            "{}\n\nPublic task:\n{}\n\nCompiler-owned immutable frame:\n{}\n\nThe transport response contract supplies the exact task-specific JSON Schema. Return schema {} and no surrounding text.\n",
            FRAMED_STAGED_MODEL_INSTRUCTION,
            task.public.task.public_specification,
            serde_json::to_string(&task.public.frame)?,
            FRAMED_STAGED_SCHEMA,
        ),
    })
}

fn prompt_for_source(task: &V2PrivateTask, surface: SurfaceName) -> String {
    let source = PrivateCorpusTask {
        public: task.public.task.clone(),
        server_task: task.server_task.clone(),
        graph_payload: task.server_task.intent.clone(),
        incremental_payload: super::corpus::incremental_from_graph_for_v2(
            &task.server_task.intent,
            8,
        ),
        staged_payload: task.staged_v1_oracle.clone(),
    };
    prompt_for(&source, surface)
}

fn audit_v2_prompts(tasks: &[V2PrivateTask]) -> AnyResult<()> {
    for task in tasks {
        let graph = serde_json::to_string(&task.server_task.intent)?;
        let inputs = serde_json::to_string(&task.server_task.inputs)?;
        let staged = serde_json::to_string(&task.staged_v1_oracle)?;
        let framed = serde_json::to_string(&task.framed_v2_oracle)?;
        for surface in V2_SURFACES {
            let prompt = prompt_v2(task, surface)?;
            if prompt.contains(&graph)
                || prompt.contains(&inputs)
                || prompt.contains(&staged)
                || prompt.contains(&framed)
            {
                return Err(format!(
                    "hidden oracle leaked into {} {} prompt",
                    task.public.task.task_id,
                    surface.name()
                )
                .into());
            }
            if surface == V2Surface::FramedStagedV2
                && (!prompt.contains(&task.public.frame.frame_hash)
                    || prompt.contains("\"choices\""))
            {
                return Err("v2 frame prompt binding or oracle-choice audit failed".into());
            }
        }
    }
    Ok(())
}

fn write_v2_prompts_and_contracts(output: &Path, tasks: &[V2PrivateTask]) -> AnyResult<()> {
    let staged_schema: Value = serde_json::from_str(agentir_authoring::STAGED_JSON_SCHEMA)?;
    for task in tasks {
        for surface in V2_SURFACES {
            let directory = output
                .join("tasks")
                .join(&task.public.task.task_id)
                .join(surface.name());
            atomic_write(
                &directory.join("prompt.txt"),
                prompt_v2(task, surface)?.as_bytes(),
            )?;
            let schema = match surface {
                V2Surface::StagedV1 => staged_schema.clone(),
                V2Surface::FramedStagedV2 => task.public.response_json_schema.clone(),
            };
            atomic_json(
                &directory.join("response-contract.json"),
                &json!({
                    "format":"agentir.authoring_eval.response_contract.v2",
                    "schema_id":match surface { V2Surface::StagedV1 => STAGED_SCHEMA, V2Surface::FramedStagedV2 => FRAMED_STAGED_SCHEMA },
                    "json_schema":schema,
                    "allow_extra_text":false,
                    "maximum_output_bytes":super::MAX_PROVIDER_RESPONSE_BYTES,
                    "structured_output_policy":"enforce_if_supported_and_report_capability",
                    "binding":{
                        "task_id":task.public.task.task_id,
                        "phase":"initial",
                        "repair_attempt":0
                    }
                }),
            )?;
        }
    }
    Ok(())
}

fn v2_corpus_hash(tasks: &[V2PrivateTask]) -> AnyResult<String> {
    let bytes = serde_json::to_vec(&json!({
        "format":"agentir.authoring_eval.ergonomics_v2.semantic_corpus",
        "format_version":V2_FORMAT_VERSION,
        "tasks":tasks,
    }))?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn write_design(output: &Path, plan: &V2ExecutionPlan, tasks: &[V2PrivateTask]) -> AnyResult<()> {
    let total_v1_fields = tasks
        .iter()
        .map(|task| task.public.ergonomics.staged_v1_model_authored_fields)
        .sum::<usize>();
    let total_v2_fields = tasks
        .iter()
        .map(|task| task.public.ergonomics.framed_v2_model_authored_fields)
        .sum::<usize>();
    let reduction = if total_v1_fields == 0 {
        0
    } else {
        (total_v1_fields.saturating_sub(total_v2_fields) as u128 * 100) / total_v1_fields as u128
    };
    let mut markdown = String::new();
    writeln!(
        markdown,
        "# AgentIR authoring ergonomics v2 offline evaluation\n"
    )?;
    writeln!(
        markdown,
        "PAID_CALLS_AUTHORIZED=false. No provider call is implemented by this plan artifact.\n"
    )?;
    writeln!(markdown, "## Predeclared plan\n")?;
    writeln!(markdown, "- Formula: {}.", plan.formula)?;
    writeln!(
        markdown,
        "- Initial upper bound: {}.",
        plan.planned_initial_calls
    )?;
    writeln!(
        markdown,
        "- Conditional one-repair upper bound: {}.",
        plan.planned_conditional_repair_calls
    )?;
    writeln!(
        markdown,
        "- Total upper bound: {}.",
        plan.planned_maximum_total_calls
    )?;
    writeln!(markdown, "- Primary metric: initial exact-intent success.")?;
    writeln!(markdown, "- Primary analysis: {}.", plan.statistical_test)?;
    writeln!(
        markdown,
        "- Secondary descriptive metrics: strict schema, local compile, final exact-intent success, publication, portable/native execution, one-repair recovery, authored bytes/fields/operations, expanded operations, latency/tokens when reported, taxonomy and first path."
    )?;
    writeln!(
        markdown,
        "- No significance claim is permitted for secondary metrics or the historical v1 observations.\n"
    )?;
    writeln!(markdown, "## Ergonomics accounting\n")?;
    writeln!(
        markdown,
        "Across the 30 oracle responses, staged v1 authors {} JSON object fields and framed v2 authors {} ({}% fewer by this predeclared recursive object-key count). Both author one semantic choice per body slot; compiler expansion remains {} total operations across the corpus.",
        total_v1_fields,
        total_v2_fields,
        reduction,
        tasks
            .iter()
            .map(|task| task.public.ergonomics.expanded_operations)
            .sum::<usize>()
    )?;
    writeln!(
        markdown,
        "The v2 model never authors stages, seed, bind names, prefix/count/stride/offset, warmup arrays, or graph indices.\n"
    )?;
    writeln!(markdown, "## Safe resume and evidence rules\n")?;
    writeln!(
        markdown,
        "Each initial cell has one immutable task/session/surface/schema binding. A repair reuses that session and additionally binds the SHA-256 of the exact previous raw payload plus the local diagnostic code/path and repair_attempt=1. Prepared state is durable before a provider call; indeterminate outcomes are not retried automatically. Raw provider bytes are atomically written before provider metadata or grading. Initial bytes are graded verbatim before any repair. Structured-output capability and enforcement must be reported by the runner; absent evidence is recorded as unreported and never claimed as enforcement.\n"
    )?;
    writeln!(markdown, "## Reproduction\n")?;
    writeln!(
        markdown,
        "    cargo run -p agentir-authoring --bin agentir-authoring-eval -- generate-v2 --output {}",
        output.display()
    )?;
    writeln!(
        markdown,
        "\nGeneration performs zero model calls. A future runner command must not be added or used until the user separately authorizes the exact call count."
    )?;
    atomic_write(&output.join("evaluation-design.md"), markdown.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_frames_lower_every_existing_public_recipe_to_the_historical_oracle() {
        let source = build_corpus().unwrap();
        let tasks = source
            .iter()
            .map(build_v2_task)
            .collect::<AnyResult<Vec<_>>>()
            .unwrap();
        assert_eq!(tasks.len(), 30);
        audit_v2_prompts(&tasks).unwrap();
        assert!(tasks.iter().all(|task| {
            compile_framed_staged(&task.public.frame, &task.framed_v2_oracle).unwrap()
                == task.server_task.intent
        }));
    }

    #[test]
    fn v2_plan_has_exact_paired_formula_and_no_call_authority() {
        let configuration = Configuration::from_environment();
        let tasks = build_corpus()
            .unwrap()
            .iter()
            .map(build_v2_task)
            .collect::<AnyResult<Vec<_>>>()
            .unwrap();
        let plan = build_v2_plan(&configuration, "hash", &tasks).unwrap();
        assert_eq!(plan.planned_initial_calls, 30 * 2 * 3 * 2);
        assert_eq!(plan.planned_conditional_repair_calls, 360);
        assert_eq!(plan.planned_maximum_total_calls, 720);
        assert!(!plan.paid_calls_authorized);
    }
}
