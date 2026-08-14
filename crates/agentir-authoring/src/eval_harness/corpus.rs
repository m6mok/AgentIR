use super::{
    AnyResult, CORPUS_SEED, DependencyStatistics, Difficulty, FORMAT_VERSION, PrivateCorpusTask,
    PublicCorpusTask, RepresentationMetadata, SURFACES, TASK_COUNT, ratio_micros,
};
use agentir_authoring::{
    AuthoringPayload, AuthoringTask, GRAPH_SCHEMA, GraphOpcode, GraphOperand, GraphOperation,
    GraphProposal, INCREMENTAL_BATCH_SCHEMA, IncrementalBatch, IncrementalOperand,
    IncrementalOperation, IncrementalTransaction, STAGED_SCHEMA, StagedOperand, StagedOperation,
    StagedProposal, TASK_SCHEMA, TRANSACTION_SCHEMA, compile_authoring_payload,
    compile_incremental_batch, compile_staged,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const HEX: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn build_corpus() -> AnyResult<Vec<PrivateCorpusTask>> {
    let operation_counts = [
        8, 12, 16, 20, 24, 28, 32, 36, 42, 48, 56, 64, 66, 72, 78, 80, 84, 88, 90, 96, 98, 100,
        104, 108, 112, 120, 121, 124, 126, 128,
    ];
    let body_sizes = [
        4, 4, 4, 4, 4, 4, 4, 4, 6, 6, 7, 8, 6, 6, 6, 8, 7, 8, 6, 8, 7, 5, 8, 6, 7, 8, 1, 4, 7, 8,
    ];
    let categories = [
        "feature_processing",
        "signal_mixing",
        "residual_pipeline",
        "telemetry_transform",
        "pricing_features",
        "ranking_features",
        "fraud_signals",
        "risk_signals",
        "stateful_filter",
        "quality_features",
    ];
    let domains = [
        "feature",
        "signal",
        "residual",
        "telemetry",
        "pricing",
        "ranking",
        "fraud",
        "risk",
        "filter",
        "quality",
    ];
    let topologies = [
        "linear_chain",
        "wide_dag",
        "fan_out",
        "repeated_operand",
        "lagged_reuse",
    ];
    let mut tasks = Vec::with_capacity(TASK_COUNT);
    for index in 0..TASK_COUNT {
        let operation_count = operation_counts[index];
        let body_size = body_sizes[index];
        if operation_count % body_size != 0 {
            return Err("corpus operation count is not divisible by body size".into());
        }
        let stages = operation_count / body_size;
        let category = categories[index % categories.len()].to_owned();
        let domain = domains[index % domains.len()];
        let topology = topologies[index % topologies.len()].to_owned();
        let task_id = format!("authoring-large-{:02}", index + 1);
        let scalar_prefix = format!("{domain}_coef");
        let tensor_prefix = format!("{domain}_input");
        let seed_name = format!("{domain}_seed");
        let scalars = (0..8)
            .map(|capture| format!("{scalar_prefix}{capture}"))
            .collect::<Vec<_>>();
        let tensors = std::iter::once(seed_name.clone())
            .chain((0..12).map(|capture| format!("{tensor_prefix}{capture}")))
            .collect::<Vec<_>>();
        let staged_payload = build_staged(
            index,
            stages,
            body_size,
            &scalar_prefix,
            &tensor_prefix,
            &seed_name,
            &topology,
        );
        let graph = independently_expand(&staged_payload)?;
        let production_graph = compile_staged(&staged_payload)?;
        if graph != production_graph {
            return Err(format!("independent oracle disagrees for {task_id}").into());
        }
        let incremental = incremental_from_graph(&graph, 1 + (index % 8));
        let incremental_graph = compile_incremental_batch(
            &incremental,
            scalars.iter().cloned(),
            tensors.iter().cloned(),
        )?;
        if incremental_graph != graph {
            return Err(format!("incremental lowering disagrees for {task_id}").into());
        }
        let server_task = AuthoringTask {
            schema: TASK_SCHEMA.to_owned(),
            task_id: task_id.clone(),
            dimension: "N".to_owned(),
            scalars: scalars.clone(),
            tensors: tensors.clone(),
            inputs: deterministic_inputs(index, &scalars, &tensors),
            intent: graph.clone(),
        };
        for payload in [
            AuthoringPayload::Graph(graph.clone()),
            AuthoringPayload::IncrementalBatch(incremental.clone()),
            AuthoringPayload::Staged(staged_payload.clone()),
        ] {
            if compile_authoring_payload(&server_task, &payload)? != graph {
                return Err(format!("surface lowering disagrees for {task_id}").into());
            }
        }
        let lags = staged_lags(&staged_payload);
        let warmups = staged_warmups(&staged_payload);
        let public_specification =
            render_public_specification(&task_id, &category, &scalars, &tensors, &staged_payload);
        validate_public_specification(&public_specification, &staged_payload, &scalars, &tensors)?;
        let incremental_max = incremental
            .transactions
            .iter()
            .map(|transaction| transaction.operations.len())
            .max()
            .unwrap_or(0);
        let public = PublicCorpusTask {
            task_id,
            category,
            difficulty: Difficulty {
                size_bucket: size_bucket(operation_count).to_owned(),
                topology,
                expanded_operations: operation_count,
                body_operations: body_size,
                stages,
                recurrence_lags: lags,
                warmup_lengths: warmups,
            },
            public_specification,
            scalars,
            tensors,
            expected_operation_count: operation_count,
            dependency_statistics: dependency_statistics(&graph),
            representations: RepresentationMetadata {
                graph_authored_operations: graph.operations.len(),
                incremental_authored_operations: graph.operations.len(),
                incremental_transactions: incremental.transactions.len(),
                incremental_max_transaction_operations: incremental_max,
                staged_authored_operations: staged_payload.body.len(),
                expanded_graph_operations: graph.operations.len(),
                staged_compression_ratio_micros: ratio_micros(
                    staged_payload.body.len(),
                    graph.operations.len(),
                ),
            },
            paired_surfaces: SURFACES.to_vec(),
        };
        tasks.push(PrivateCorpusTask {
            public,
            server_task,
            graph_payload: graph,
            incremental_payload: incremental,
            staged_payload,
        });
    }
    validate_corpus_invariants(&tasks)?;
    Ok(tasks)
}

fn build_staged(
    task_index: usize,
    stages: usize,
    body_size: usize,
    scalar_prefix: &str,
    tensor_prefix: &str,
    seed_name: &str,
    topology: &str,
) -> StagedProposal {
    let mut body = Vec::with_capacity(body_size);
    let primary_lag = 1 + (task_index % 7);
    for operation_index in 0..body_size {
        let bind = format!("$role{operation_index}");
        let operation = if operation_index == 0 {
            if body_size == 1 || task_index % 3 == 0 {
                StagedOperation {
                    bind,
                    op: GraphOpcode::Add,
                    operands: vec![
                        StagedOperand::StatePrev,
                        tensor_cycle(tensor_prefix, task_index, 1),
                    ],
                }
            } else {
                StagedOperation {
                    bind,
                    op: GraphOpcode::Fma,
                    operands: vec![
                        scalar_cycle(scalar_prefix, task_index, 0),
                        StagedOperand::StatePrev,
                        tensor_cycle(tensor_prefix, task_index, 1),
                    ],
                }
            }
        } else if body_size >= 6 && operation_index == body_size - 1 {
            let (lag, warmup) = if task_index == 19 {
                (3, 4)
            } else {
                let lag = primary_lag.min(stages.saturating_sub(1).max(1));
                let extra = usize::from(task_index % 4 == 0);
                (lag, (lag + extra).min(stages.saturating_sub(1).max(lag)))
            };
            StagedOperation {
                bind,
                op: GraphOpcode::Add,
                operands: vec![
                    stage_local(operation_index - 1),
                    state_lag(tensor_prefix, lag, warmup),
                ],
            }
        } else if body_size == 8 && operation_index == 5 && task_index % 2 == 0 {
            let lag = (1 + ((task_index + 3) % 7)).min(stages.saturating_sub(1).max(1));
            StagedOperation {
                bind,
                op: GraphOpcode::Fma,
                operands: vec![
                    stage_local(operation_index - 1),
                    scalar_cycle(scalar_prefix, task_index, operation_index),
                    state_lag(tensor_prefix, lag, lag),
                ],
            }
        } else {
            match topology {
                "linear_chain" => chain_operation(
                    bind,
                    operation_index,
                    task_index,
                    scalar_prefix,
                    tensor_prefix,
                ),
                "wide_dag" => wide_operation(
                    bind,
                    operation_index,
                    task_index,
                    scalar_prefix,
                    tensor_prefix,
                ),
                "fan_out" => fan_out_operation(
                    bind,
                    operation_index,
                    task_index,
                    scalar_prefix,
                    tensor_prefix,
                ),
                "repeated_operand" => StagedOperation {
                    bind,
                    op: if operation_index % 2 == 0 {
                        GraphOpcode::Add
                    } else {
                        GraphOpcode::Mul
                    },
                    operands: vec![
                        stage_local(operation_index - 1),
                        stage_local(operation_index - 1),
                    ],
                },
                _ => reuse_operation(
                    bind,
                    operation_index,
                    task_index,
                    scalar_prefix,
                    tensor_prefix,
                ),
            }
        };
        body.push(operation);
    }
    let state_offset = if body_size >= 4 && task_index % 6 == 1 {
        body_size - 2
    } else {
        body_size - 1
    };
    StagedProposal {
        schema: STAGED_SCHEMA.to_owned(),
        stages,
        seed: GraphOperand::Tensor {
            name: seed_name.to_owned(),
        },
        body,
        state: format!("$role{state_offset}"),
    }
}

fn chain_operation(
    bind: String,
    operation_index: usize,
    task_index: usize,
    scalar_prefix: &str,
    tensor_prefix: &str,
) -> StagedOperation {
    match operation_index % 3 {
        0 => StagedOperation {
            bind,
            op: GraphOpcode::Fma,
            operands: vec![
                scalar_cycle(scalar_prefix, task_index, operation_index),
                stage_local(operation_index - 1),
                tensor_cycle(tensor_prefix, task_index, operation_index),
            ],
        },
        1 => StagedOperation {
            bind,
            op: GraphOpcode::Mul,
            operands: vec![
                tensor_cycle(tensor_prefix, task_index, operation_index),
                stage_local(operation_index - 1),
            ],
        },
        _ => StagedOperation {
            bind,
            op: GraphOpcode::Add,
            operands: vec![
                stage_local(operation_index - 1),
                tensor_cycle(tensor_prefix, task_index, operation_index),
            ],
        },
    }
}

fn wide_operation(
    bind: String,
    operation_index: usize,
    task_index: usize,
    scalar_prefix: &str,
    tensor_prefix: &str,
) -> StagedOperation {
    if operation_index % 2 == 0 {
        StagedOperation {
            bind,
            op: GraphOpcode::Fma,
            operands: vec![
                stage_local(0),
                scalar_cycle(scalar_prefix, task_index, operation_index),
                tensor_cycle(tensor_prefix, task_index, operation_index),
            ],
        }
    } else {
        StagedOperation {
            bind,
            op: GraphOpcode::Add,
            operands: vec![
                tensor_cycle(tensor_prefix, task_index, operation_index),
                stage_local(0),
            ],
        }
    }
}

fn fan_out_operation(
    bind: String,
    operation_index: usize,
    task_index: usize,
    scalar_prefix: &str,
    tensor_prefix: &str,
) -> StagedOperation {
    if operation_index == 1 {
        StagedOperation {
            bind,
            op: GraphOpcode::Mul,
            operands: vec![tensor_cycle(tensor_prefix, task_index, 7), stage_local(0)],
        }
    } else if operation_index == 2 {
        StagedOperation {
            bind,
            op: GraphOpcode::Add,
            operands: vec![stage_local(0), tensor_cycle(tensor_prefix, task_index, 8)],
        }
    } else {
        StagedOperation {
            bind,
            op: GraphOpcode::Fma,
            operands: vec![
                stage_local(operation_index - 1),
                scalar_cycle(scalar_prefix, task_index, operation_index),
                stage_local(operation_index.saturating_sub(2)),
            ],
        }
    }
}

fn reuse_operation(
    bind: String,
    operation_index: usize,
    task_index: usize,
    scalar_prefix: &str,
    tensor_prefix: &str,
) -> StagedOperation {
    match operation_index % 4 {
        1 => StagedOperation {
            bind,
            op: GraphOpcode::Mul,
            operands: vec![tensor_cycle(tensor_prefix, task_index, 3), stage_local(0)],
        },
        2 => StagedOperation {
            bind,
            op: GraphOpcode::Add,
            operands: vec![stage_local(0), tensor_cycle(tensor_prefix, task_index, 4)],
        },
        3 => StagedOperation {
            bind,
            op: GraphOpcode::Add,
            operands: vec![
                stage_local(operation_index - 2),
                stage_local(operation_index - 1),
            ],
        },
        _ => StagedOperation {
            bind,
            op: GraphOpcode::Fma,
            operands: vec![
                stage_local(operation_index - 1),
                scalar_cycle(scalar_prefix, task_index, operation_index),
                StagedOperand::StatePrev,
            ],
        },
    }
}

fn scalar_cycle(prefix: &str, task_index: usize, operation_index: usize) -> StagedOperand {
    StagedOperand::ScalarCycle {
        prefix: prefix.to_owned(),
        count: 8,
        stride: 1 + ((task_index + operation_index) % 7),
        offset: (task_index * 3 + operation_index) % 8,
    }
}

fn tensor_cycle(prefix: &str, task_index: usize, operation_index: usize) -> StagedOperand {
    StagedOperand::TensorCycle {
        prefix: prefix.to_owned(),
        count: 12,
        stride: 1 + ((task_index * 2 + operation_index) % 11),
        offset: (task_index + operation_index * 3) % 12,
    }
}

fn stage_local(index: usize) -> StagedOperand {
    StagedOperand::StageLocal {
        name: format!("$role{index}"),
    }
}

fn state_lag(tensor_prefix: &str, lag: usize, warmup: usize) -> StagedOperand {
    StagedOperand::StateLag {
        stages: lag,
        initial: (0..warmup)
            .map(|index| GraphOperand::Tensor {
                name: format!("{tensor_prefix}{}", 11 - (index % 12)),
            })
            .collect(),
    }
}

fn independently_expand(source: &StagedProposal) -> AnyResult<GraphProposal> {
    if source.stages == 0 || source.body.is_empty() || source.body.len() > 8 {
        return Err("invalid staged oracle source".into());
    }
    let expanded = source
        .stages
        .checked_mul(source.body.len())
        .ok_or("staged oracle expansion overflow")?;
    if expanded > 128 {
        return Err("staged oracle exceeds 128 operations".into());
    }
    let bindings = source
        .body
        .iter()
        .enumerate()
        .map(|(index, operation)| (operation.bind.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let state_offset = *bindings
        .get(source.state.as_str())
        .ok_or("unknown staged state")?;
    let mut operations = Vec::with_capacity(expanded);
    let mut states = Vec::with_capacity(source.stages);
    for stage in 0..source.stages {
        let base = operations.len();
        for operation in &source.body {
            let operands = operation
                .operands
                .iter()
                .map(|operand| expand_operand(operand, source, &bindings, &states, stage, base))
                .collect::<AnyResult<Vec<_>>>()?;
            operations.push(GraphOperation {
                op: operation.op,
                operands,
            });
        }
        states.push(base + state_offset);
    }
    Ok(GraphProposal {
        schema: GRAPH_SCHEMA.to_owned(),
        operations,
        r#yield: *states.last().ok_or("no staged state")?,
    })
}

fn expand_operand(
    operand: &StagedOperand,
    source: &StagedProposal,
    bindings: &BTreeMap<&str, usize>,
    states: &[usize],
    stage: usize,
    base: usize,
) -> AnyResult<GraphOperand> {
    Ok(match operand {
        StagedOperand::Scalar { name } => GraphOperand::Scalar { name: name.clone() },
        StagedOperand::Tensor { name } => GraphOperand::Tensor { name: name.clone() },
        StagedOperand::StageLocal { name } => GraphOperand::Local {
            operation: base + bindings[name.as_str()],
        },
        StagedOperand::StatePrev => {
            if stage == 0 {
                source.seed.clone()
            } else {
                GraphOperand::Local {
                    operation: states[stage - 1],
                }
            }
        }
        StagedOperand::StateLag { stages, initial } => {
            if *stages == 0 || initial.len() < *stages {
                return Err("invalid state lag".into());
            }
            if stage < initial.len() {
                initial[stage].clone()
            } else {
                GraphOperand::Local {
                    operation: states[stage - stages],
                }
            }
        }
        StagedOperand::ScalarCycle {
            prefix,
            count,
            stride,
            offset,
        } => GraphOperand::Scalar {
            name: format!(
                "{prefix}{}",
                widened_cycle(stage, *stride, *offset, *count)?
            ),
        },
        StagedOperand::TensorCycle {
            prefix,
            count,
            stride,
            offset,
        } => GraphOperand::Tensor {
            name: format!(
                "{prefix}{}",
                widened_cycle(stage, *stride, *offset, *count)?
            ),
        },
    })
}

fn widened_cycle(stage: usize, stride: usize, offset: usize, count: usize) -> AnyResult<usize> {
    if count == 0 {
        return Err("zero capture cycle".into());
    }
    let index = ((stage as u128) * (stride as u128) + (offset as u128)) % (count as u128);
    Ok(usize::try_from(index)?)
}

fn incremental_from_graph(graph: &GraphProposal, transaction_size: usize) -> IncrementalBatch {
    let mut transactions = Vec::new();
    let mut base = 0;
    while base < graph.operations.len() {
        let end = (base + transaction_size).min(graph.operations.len());
        let operations = graph.operations[base..end]
            .iter()
            .enumerate()
            .map(|(offset, operation)| IncrementalOperation {
                bind: format!("$v{}", base + offset),
                op: operation.op,
                operands: operation
                    .operands
                    .iter()
                    .map(|operand| match operand {
                        GraphOperand::Scalar { name } => {
                            IncrementalOperand::Scalar { name: name.clone() }
                        }
                        GraphOperand::Tensor { name } => {
                            IncrementalOperand::Tensor { name: name.clone() }
                        }
                        GraphOperand::Local { operation } => IncrementalOperand::Local {
                            name: format!("$v{operation}"),
                        },
                    })
                    .collect(),
            })
            .collect();
        transactions.push(IncrementalTransaction {
            schema: TRANSACTION_SCHEMA.to_owned(),
            base_operations: base,
            operations,
        });
        base = end;
    }
    IncrementalBatch {
        schema: INCREMENTAL_BATCH_SCHEMA.to_owned(),
        transactions,
        r#yield: format!("$v{}", graph.r#yield),
    }
}

pub(crate) fn incremental_from_graph_for_v2(
    graph: &GraphProposal,
    transaction_size: usize,
) -> IncrementalBatch {
    incremental_from_graph(graph, transaction_size)
}

fn deterministic_inputs(
    task_index: usize,
    scalars: &[String],
    tensors: &[String],
) -> BTreeMap<String, Value> {
    let mut inputs = BTreeMap::new();
    for (index, name) in scalars.iter().enumerate() {
        let value = 0.03125 + ((task_index + index) % 13) as f64 * 0.001_953_125;
        inputs.insert(name.clone(), json!(value));
    }
    for (index, name) in tensors.iter().enumerate() {
        let base = 0.007_812_5 + ((task_index * 3 + index) % 17) as f64 * 0.000_976_562_5;
        inputs.insert(
            name.clone(),
            json!([
                base,
                base + 0.000_244_140_625,
                base + 0.000_488_281_25,
                base + 0.000_732_421_875
            ]),
        );
    }
    inputs
}

fn render_public_specification(
    task_id: &str,
    category: &str,
    scalars: &[String],
    tensors: &[String],
    staged: &StagedProposal,
) -> String {
    let mut lines = vec![
        format!("Task {task_id}. Category: {category}."),
        "Author one exact bounded one-dimensional f32 elementwise component.".to_owned(),
        format!(
            "Available scalar captures in exact declaration order: {}.",
            scalars.join(", ")
        ),
        format!(
            "Available tensor captures in exact declaration order: {}.",
            tensors.join(", ")
        ),
        format!(
            "There are exactly {} stages numbered 0 through {}; each stage has exactly {} operations and stages are expanded in increasing order.",
            staged.stages,
            staged.stages - 1,
            staged.body.len()
        ),
        format!(
            "Before stage 0, the state is {}.",
            render_graph_operand(&staged.seed)
        ),
        "In every stage i, execute these operations in this exact order; role names describe dependencies and are not extra operations:".to_owned(),
    ];
    for (index, operation) in staged.body.iter().enumerate() {
        let operands = operation
            .operands
            .iter()
            .map(render_staged_operand)
            .collect::<Vec<_>>()
            .join(" ; ");
        lines.push(format!(
            "{}. role{}_i = {}({}).",
            index + 1,
            index,
            opcode_name(operation.op),
            operands
        ));
    }
    let state = staged
        .body
        .iter()
        .position(|operation| operation.bind == staged.state)
        .expect("known state");
    lines.push(format!(
        "The state produced by stage i is role{state}_i. The final yield is role{state}_{} from the final stage.",
        staged.stages - 1
    ));
    for (number, (lag, initial)) in staged
        .body
        .iter()
        .flat_map(|operation| &operation.operands)
        .filter_map(|operand| match operand {
            StagedOperand::StateLag { stages, initial } => Some((*stages, initial)),
            _ => None,
        })
        .enumerate()
    {
        let warmup = initial
            .iter()
            .map(render_graph_operand)
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "Lag rule {}: lag is {}; stages 0 through {} use the complete warmup sequence [{}] in order, and stage {} first uses state_({}-{}) from stage {}.",
            number + 1,
            lag,
            initial.len() - 1,
            warmup,
            initial.len(),
            initial.len(),
            lag,
            initial.len() - lag
        ));
    }
    lines.push(format!(
        "The expanded graph therefore has exactly {} operations. Do not add operations, omit operations, recompute a named role, change operand order, decompose fma, invent captures, or invent names.",
        staged.stages * staged.body.len()
    ));
    lines.join("\n")
}

fn render_graph_operand(operand: &GraphOperand) -> String {
    match operand {
        GraphOperand::Scalar { name } => format!("scalar capture {name}"),
        GraphOperand::Tensor { name } => format!("tensor capture {name}"),
        GraphOperand::Local { operation } => format!("forbidden local {operation}"),
    }
}

fn render_staged_operand(operand: &StagedOperand) -> String {
    match operand {
        StagedOperand::Scalar { name } => format!("scalar capture {name}"),
        StagedOperand::Tensor { name } => format!("tensor capture {name}"),
        StagedOperand::StageLocal { name } => {
            format!("same-stage {}", name.trim_start_matches('$'))
        }
        StagedOperand::StatePrev => {
            "previous-stage state, using the declared seed at stage 0".to_owned()
        }
        StagedOperand::StateLag { stages, initial } => format!(
            "state lag {stages} with warmup [{}]",
            initial
                .iter()
                .map(render_graph_operand)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        StagedOperand::ScalarCycle {
            prefix,
            count,
            stride,
            offset,
        } => format!("scalar capture {prefix}((i*{stride}+{offset}) mod {count})"),
        StagedOperand::TensorCycle {
            prefix,
            count,
            stride,
            offset,
        } => format!("tensor capture {prefix}((i*{stride}+{offset}) mod {count})"),
    }
}

const fn opcode_name(opcode: GraphOpcode) -> &'static str {
    match opcode {
        GraphOpcode::Add => "add",
        GraphOpcode::Mul => "mul",
        GraphOpcode::Fma => "fma",
    }
}

fn validate_public_specification(
    specification: &str,
    staged: &StagedProposal,
    scalars: &[String],
    tensors: &[String],
) -> AnyResult<()> {
    if specification.contains(TASK_SCHEMA)
        || specification.contains("\"operations\"")
        || specification.contains("\"schema\"")
        || specification.contains("$role")
    {
        return Err("public specification leaked a payload or hidden envelope".into());
    }
    for name in scalars.iter().chain(tensors) {
        if !specification.contains(name) {
            return Err(format!("public specification omits capture {name}").into());
        }
    }
    for index in 0..staged.body.len() {
        if specification.matches(&format!("role{index}_i =")).count() != 1 {
            return Err(format!("public specification does not define role{index} once").into());
        }
    }
    for operation in &staged.body {
        for operand in &operation.operands {
            if let StagedOperand::StageLocal { name } = operand {
                let prior = name
                    .trim_start_matches("$role")
                    .parse::<usize>()
                    .map_err(|_| "invalid role binding")?;
                let current = operation
                    .bind
                    .trim_start_matches("$role")
                    .parse::<usize>()
                    .map_err(|_| "invalid role binding")?;
                if prior >= current {
                    return Err("public recipe contains a forward same-stage reference".into());
                }
            }
        }
    }
    if specification.matches("The final yield is ").count() != 1
        || !specification.contains(&format!(
            "exactly {} operations",
            staged.stages * staged.body.len()
        ))
    {
        return Err("public specification lacks a unique count or final yield".into());
    }
    Ok(())
}

fn dependency_statistics(graph: &GraphProposal) -> DependencyStatistics {
    let mut uses = vec![0_usize; graph.operations.len()];
    let mut local_references = 0;
    let mut maximum_reference_distance = 0;
    let mut repeated_operand_operations = 0;
    let mut fma_operations = 0;
    for (index, operation) in graph.operations.iter().enumerate() {
        fma_operations += usize::from(operation.op == GraphOpcode::Fma);
        let mut seen = BTreeSet::new();
        let mut repeated = false;
        for operand in &operation.operands {
            if !seen.insert(serde_json::to_string(operand).expect("operand serialization")) {
                repeated = true;
            }
            if let GraphOperand::Local { operation } = operand {
                local_references += 1;
                uses[*operation] += 1;
                maximum_reference_distance = maximum_reference_distance.max(index - operation);
            }
        }
        repeated_operand_operations += usize::from(repeated);
    }
    DependencyStatistics {
        local_references,
        maximum_reference_distance,
        maximum_fan_out: uses.iter().copied().max().unwrap_or(0),
        reused_local_values: uses.iter().filter(|count| **count > 1).count(),
        repeated_operand_operations,
        fma_operations,
        non_final_yield: graph.r#yield + 1 != graph.operations.len(),
    }
}

fn staged_lags(staged: &StagedProposal) -> Vec<usize> {
    staged
        .body
        .iter()
        .flat_map(|operation| &operation.operands)
        .filter_map(|operand| match operand {
            StagedOperand::StateLag { stages, .. } => Some(*stages),
            _ => None,
        })
        .collect()
}

fn staged_warmups(staged: &StagedProposal) -> Vec<usize> {
    staged
        .body
        .iter()
        .flat_map(|operation| &operation.operands)
        .filter_map(|operand| match operand {
            StagedOperand::StateLag { initial, .. } => Some(initial.len()),
            _ => None,
        })
        .collect()
}

fn size_bucket(operations: usize) -> &'static str {
    match operations {
        8..=24 => "small",
        25..=64 => "medium",
        65..=96 => "large",
        97..=120 => "very_large",
        121..=128 => "boundary",
        _ => "out_of_scope",
    }
}

fn validate_corpus_invariants(tasks: &[PrivateCorpusTask]) -> AnyResult<()> {
    if tasks.len() != TASK_COUNT {
        return Err(format!("expected {TASK_COUNT} tasks, got {}", tasks.len()).into());
    }
    let distribution = tasks.iter().fold(BTreeMap::new(), |mut counts, task| {
        *counts
            .entry(task.public.difficulty.size_bucket.as_str())
            .or_insert(0_usize) += 1;
        counts
    });
    let expected = BTreeMap::from([
        ("small", 5),
        ("medium", 7),
        ("large", 8),
        ("very_large", 6),
        ("boundary", 4),
    ]);
    if distribution != expected {
        return Err(format!("wrong size distribution: {distribution:?}").into());
    }
    if tasks
        .iter()
        .filter(|task| task.public.paired_surfaces.len() == 3)
        .count()
        < 20
    {
        return Err("fewer than 20 paired tasks".into());
    }
    if !tasks
        .iter()
        .any(|task| task.public.expected_operation_count == 128)
    {
        return Err("missing exact 128-operation task".into());
    }
    if !tasks.iter().any(|task| {
        task.public
            .difficulty
            .recurrence_lags
            .iter()
            .zip(&task.public.difficulty.warmup_lengths)
            .any(|(lag, warmup)| *lag == 3 && *warmup == 4)
    }) {
        return Err("missing four-warmup/lag-three task".into());
    }
    let covered_lags = tasks
        .iter()
        .flat_map(|task| &task.public.difficulty.recurrence_lags)
        .copied()
        .collect::<BTreeSet<_>>();
    if !(1..=7).all(|lag| covered_lags.contains(&lag)) {
        return Err(format!("lag coverage incomplete: {covered_lags:?}").into());
    }
    if !tasks
        .iter()
        .any(|task| task.public.difficulty.recurrence_lags.len() > 1)
    {
        return Err("missing multiple-lag body".into());
    }
    if !tasks
        .iter()
        .any(|task| task.public.dependency_statistics.non_final_yield)
    {
        return Err("missing non-final yield".into());
    }
    Ok(())
}

pub(crate) fn corpus_hash(tasks: &[PrivateCorpusTask]) -> AnyResult<String> {
    let bytes = serde_json::to_vec(&json!({
        "format":"agentir.authoring_eval.semantic_corpus",
        "format_version":FORMAT_VERSION,
        "seed":CORPUS_SEED,
        "tasks":tasks,
    }))?;
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(encoded)
}

pub(crate) fn prompt_for(task: &PrivateCorpusTask, surface: super::SurfaceName) -> String {
    format!(
        "{}\n\nPublic task:\n{}\n\nReturn exactly one JSON payload for schema {} and no surrounding text.\n",
        surface.sdk().model_instruction(),
        task.public.public_specification,
        surface.sdk().schema()
    )
}

pub(crate) fn audit_prompts(tasks: &[PrivateCorpusTask]) -> AnyResult<()> {
    for task in tasks {
        let graph = serde_json::to_string(&task.graph_payload)?;
        let incremental = serde_json::to_string(&task.incremental_payload)?;
        let staged = serde_json::to_string(&task.staged_payload)?;
        let inputs = serde_json::to_string(&task.server_task.inputs)?;
        for surface in SURFACES {
            let prompt = prompt_for(task, surface);
            if prompt.contains(TASK_SCHEMA)
                || prompt.contains(&graph)
                || prompt.contains(&incremental)
                || prompt.contains(&staged)
                || prompt.contains(&inputs)
            {
                return Err(format!(
                    "hidden oracle or server envelope leaked into {} {} prompt",
                    task.public.task_id,
                    surface.directory()
                )
                .into());
            }
        }
    }
    Ok(())
}

pub(crate) fn write_prompts(output: &Path, tasks: &[PrivateCorpusTask]) -> AnyResult<()> {
    for task in tasks {
        for surface in SURFACES {
            super::atomic_write(
                &output
                    .join("prompts")
                    .join(&task.public.task_id)
                    .join(format!("{}.txt", surface.directory())),
                prompt_for(task, surface).as_bytes(),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_has_exact_distribution_and_independent_lowerings() {
        let tasks = build_corpus().expect("valid corpus");
        assert_eq!(tasks.len(), 30);
        assert_eq!(tasks.last().unwrap().graph_payload.operations.len(), 128);
        assert!(tasks.iter().all(|task| {
            independently_expand(&task.staged_payload).unwrap() == task.graph_payload
        }));
    }

    #[test]
    fn prompts_do_not_contain_private_envelopes_or_payloads() {
        let tasks = build_corpus().unwrap();
        audit_prompts(&tasks).unwrap();
        for task in tasks {
            for surface in SURFACES {
                let prompt = prompt_for(&task, surface);
                assert!(!prompt.contains(TASK_SCHEMA));
                assert!(
                    !prompt.contains(&serde_json::to_string(&task.server_task.inputs).unwrap())
                );
            }
        }
    }
}
