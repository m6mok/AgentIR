use super::grader::GradeRecord;
use super::runner::ProviderMetadata;
use super::{
    AnyResult, Configuration, ExecutionPlan, PublicCorpus, atomic_json, atomic_write,
    attempt_directory, ratio_micros, read_json,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct MetricCounts {
    attempted_cells: usize,
    completed_initial_calls: usize,
    provider_failures: usize,
    harness_failures: usize,
    model_correctness_denominator: usize,
    provider_inclusive_denominator: usize,
    strict_schema_successes: usize,
    local_compile_successes: usize,
    exact_intent_successes: usize,
    publication_successes: usize,
    portable_execution_successes: usize,
    native_execution_successes: usize,
    repair_attempts: usize,
    repair_correctness_denominator: usize,
    repair_recoveries: usize,
    final_correctness_denominator: usize,
    final_successes: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LatencySummary {
    samples: usize,
    mean_ms: Option<u64>,
    median_ms: Option<u64>,
    p95_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Summary {
    format: String,
    format_version: u32,
    corpus_hash: String,
    experiment_status: String,
    planned_initial_calls: usize,
    planned_maximum_total_calls: usize,
    counts: MetricCounts,
    strict_schema_success_rate_micros: Option<u64>,
    local_compile_success_rate_micros: Option<u64>,
    initial_exact_intent_success_rate_micros: Option<u64>,
    repaired_recovery_rate_micros: Option<u64>,
    final_success_rate_micros: Option<u64>,
    latency: LatencySummary,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    authored_json_bytes: u64,
    authored_operation_count: u64,
    expanded_graph_operation_count: u64,
    staged_mean_compression_ratio_micros: Option<u64>,
    error_taxonomy: BTreeMap<String, usize>,
    first_failing_json_paths: BTreeMap<String, usize>,
    by_model: BTreeMap<String, MetricCounts>,
    by_reasoning_level: BTreeMap<String, MetricCounts>,
    by_surface: BTreeMap<String, MetricCounts>,
    by_size_bucket: BTreeMap<String, MetricCounts>,
    by_category: BTreeMap<String, MetricCounts>,
    by_topology: BTreeMap<String, MetricCounts>,
    paired_per_task_initial_exact_successes: BTreeMap<String, BTreeMap<String, usize>>,
    limitations: Vec<String>,
}

pub(crate) fn build_summary(
    output: &Path,
    public: &PublicCorpus,
    plan: &ExecutionPlan,
    status: &str,
) -> AnyResult<Summary> {
    let task_metadata = public
        .tasks
        .iter()
        .map(|task| (task.task_id.as_str(), task))
        .collect::<BTreeMap<_, _>>();
    let mut counts = MetricCounts {
        provider_inclusive_denominator: plan.cells.len(),
        ..MetricCounts::default()
    };
    let mut by_model = BTreeMap::new();
    let mut by_reasoning = BTreeMap::new();
    let mut by_surface = BTreeMap::new();
    let mut by_size = BTreeMap::new();
    let mut by_category = BTreeMap::new();
    let mut by_topology = BTreeMap::new();
    let mut paired = BTreeMap::new();
    let mut taxonomy = BTreeMap::new();
    let mut paths = BTreeMap::new();
    let mut latencies = Vec::new();
    let mut input_tokens = 0_u64;
    let mut output_tokens = 0_u64;
    let mut has_input_tokens = false;
    let mut has_output_tokens = false;
    let mut authored_bytes = 0_u64;
    let mut authored_operations = 0_u64;
    let mut expanded_operations = 0_u64;
    let mut staged_ratios = Vec::new();
    for cell in &plan.cells {
        let task = task_metadata[&*cell.task_id];
        ensure_dimension(&mut by_model, &cell.model);
        ensure_dimension(&mut by_reasoning, &cell.reasoning_level);
        ensure_dimension(&mut by_surface, cell.surface.directory());
        ensure_dimension(&mut by_size, &task.difficulty.size_bucket);
        ensure_dimension(&mut by_category, &task.category);
        ensure_dimension(&mut by_topology, &task.difficulty.topology);
        for dimension in dimension_counts_mut(
            &mut by_model,
            &mut by_reasoning,
            &mut by_surface,
            &mut by_size,
            &mut by_category,
            &mut by_topology,
            cell,
            task,
        ) {
            dimension.provider_inclusive_denominator += 1;
        }
        let directory = attempt_directory(output, cell);
        let provider_path = directory.join("initial-provider.json");
        let mut provider_attempted = false;
        if provider_path.exists() {
            provider_attempted = true;
            let provider: ProviderMetadata = read_json(&provider_path)?;
            collect_provider_metrics(
                &provider,
                &mut latencies,
                &mut input_tokens,
                &mut output_tokens,
                &mut has_input_tokens,
                &mut has_output_tokens,
            );
            if !provider.completed() {
                counts.provider_failures += 1;
                for dimension in dimension_counts_mut(
                    &mut by_model,
                    &mut by_reasoning,
                    &mut by_surface,
                    &mut by_size,
                    &mut by_category,
                    &mut by_topology,
                    cell,
                    task,
                ) {
                    dimension.provider_failures += 1;
                }
            }
        }
        let initial_path = directory.join("initial-grade.json");
        if !initial_path.exists() {
            if provider_attempted {
                counts.attempted_cells += 1;
                for dimension in dimension_counts_mut(
                    &mut by_model,
                    &mut by_reasoning,
                    &mut by_surface,
                    &mut by_size,
                    &mut by_category,
                    &mut by_topology,
                    cell,
                    task,
                ) {
                    dimension.attempted_cells += 1;
                }
            }
            continue;
        }
        let initial: GradeRecord = read_json(&initial_path)?;
        counts.attempted_cells += 1;
        counts.completed_initial_calls += 1;
        counts.model_correctness_denominator += usize::from(!initial.harness_error);
        add_initial_grade(&mut counts, &initial);
        for dimension in dimension_counts_mut(
            &mut by_model,
            &mut by_reasoning,
            &mut by_surface,
            &mut by_size,
            &mut by_category,
            &mut by_topology,
            cell,
            task,
        ) {
            dimension.attempted_cells += 1;
            dimension.completed_initial_calls += 1;
            dimension.model_correctness_denominator += usize::from(!initial.harness_error);
            add_initial_grade(dimension, &initial);
        }
        collect_grade_metrics(
            &initial,
            &mut taxonomy,
            &mut paths,
            &mut authored_bytes,
            &mut authored_operations,
            &mut expanded_operations,
            &mut staged_ratios,
        );
        if initial.exact_intent_success {
            *paired
                .entry(cell.task_id.clone())
                .or_insert_with(BTreeMap::new)
                .entry(cell.surface.directory().to_owned())
                .or_insert(0) += 1;
        }
        let repair_provider_path = directory.join("repair-provider.json");
        if repair_provider_path.exists() {
            let provider: ProviderMetadata = read_json(&repair_provider_path)?;
            collect_provider_metrics(
                &provider,
                &mut latencies,
                &mut input_tokens,
                &mut output_tokens,
                &mut has_input_tokens,
                &mut has_output_tokens,
            );
            if !provider.completed() {
                counts.provider_failures += 1;
                for dimension in dimension_counts_mut(
                    &mut by_model,
                    &mut by_reasoning,
                    &mut by_surface,
                    &mut by_size,
                    &mut by_category,
                    &mut by_topology,
                    cell,
                    task,
                ) {
                    dimension.provider_failures += 1;
                }
            }
        }
        let repair_path = directory.join("repair-grade.json");
        if repair_path.exists() {
            let repair: GradeRecord = read_json(&repair_path)?;
            counts.repair_attempts += 1;
            counts.repair_correctness_denominator += usize::from(!repair.harness_error);
            counts.final_correctness_denominator += usize::from(!repair.harness_error);
            counts.harness_failures += usize::from(repair.harness_error);
            counts.repair_recoveries += usize::from(repair.final_success);
            counts.final_successes += usize::from(repair.final_success);
            for dimension in dimension_counts_mut(
                &mut by_model,
                &mut by_reasoning,
                &mut by_surface,
                &mut by_size,
                &mut by_category,
                &mut by_topology,
                cell,
                task,
            ) {
                dimension.repair_attempts += 1;
                dimension.repair_correctness_denominator += usize::from(!repair.harness_error);
                dimension.final_correctness_denominator += usize::from(!repair.harness_error);
                dimension.harness_failures += usize::from(repair.harness_error);
                dimension.repair_recoveries += usize::from(repair.final_success);
                dimension.final_successes += usize::from(repair.final_success);
            }
            collect_grade_metrics(
                &repair,
                &mut taxonomy,
                &mut paths,
                &mut authored_bytes,
                &mut authored_operations,
                &mut expanded_operations,
                &mut staged_ratios,
            );
        } else if initial.final_success {
            counts.final_correctness_denominator += 1;
            counts.final_successes += 1;
            for dimension in dimension_counts_mut(
                &mut by_model,
                &mut by_reasoning,
                &mut by_surface,
                &mut by_size,
                &mut by_category,
                &mut by_topology,
                cell,
                task,
            ) {
                dimension.final_correctness_denominator += 1;
                dimension.final_successes += 1;
            }
        }
    }
    latencies.sort_unstable();
    let latency = LatencySummary {
        samples: latencies.len(),
        mean_ms: (!latencies.is_empty()).then(|| {
            latencies.iter().copied().sum::<u64>() / u64::try_from(latencies.len()).unwrap_or(1)
        }),
        median_ms: percentile(&latencies, 50),
        p95_ms: percentile(&latencies, 95),
    };
    Ok(Summary {
        format: "agentir.authoring_eval.summary".to_owned(),
        format_version: super::FORMAT_VERSION,
        corpus_hash: public.corpus_hash.clone(),
        experiment_status: status.to_owned(),
        planned_initial_calls: plan.planned_initial_calls,
        planned_maximum_total_calls: plan.planned_maximum_total_calls,
        strict_schema_success_rate_micros: rate_micros(
            counts.strict_schema_successes,
            counts.model_correctness_denominator,
        ),
        local_compile_success_rate_micros: rate_micros(
            counts.local_compile_successes,
            counts.model_correctness_denominator,
        ),
        initial_exact_intent_success_rate_micros: rate_micros(
            counts.exact_intent_successes,
            counts.model_correctness_denominator,
        ),
        repaired_recovery_rate_micros: rate_micros(
            counts.repair_recoveries,
            counts.repair_correctness_denominator,
        ),
        final_success_rate_micros: rate_micros(
            counts.final_successes,
            counts.final_correctness_denominator,
        ),
        counts,
        latency,
        input_tokens: has_input_tokens.then_some(input_tokens),
        output_tokens: has_output_tokens.then_some(output_tokens),
        authored_json_bytes: authored_bytes,
        authored_operation_count: authored_operations,
        expanded_graph_operation_count: expanded_operations,
        staged_mean_compression_ratio_micros: (!staged_ratios.is_empty()).then(|| {
            staged_ratios.iter().copied().sum::<u64>()
                / u64::try_from(staged_ratios.len()).unwrap_or(1)
        }),
        error_taxonomy: taxonomy,
        first_failing_json_paths: paths,
        by_model,
        by_reasoning_level: by_reasoning,
        by_surface,
        by_size_bucket: by_size,
        by_category,
        by_topology,
        paired_per_task_initial_exact_successes: paired,
        limitations: vec![
            "This evaluates bounded one-dimensional f32 elementwise authoring components, not all AgentIR programs or arbitrary distributed systems.".to_owned(),
            "Execution agreement is confidence evidence; exact hidden graph equality is the intent metric.".to_owned(),
            "Observed differences are descriptive unless a separate predeclared statistical analysis is added.".to_owned(),
        ],
    })
}

fn ensure_dimension(map: &mut BTreeMap<String, MetricCounts>, key: &str) {
    map.entry(key.to_owned()).or_default();
}

#[allow(clippy::too_many_arguments)]
fn dimension_counts_mut<'a>(
    by_model: &'a mut BTreeMap<String, MetricCounts>,
    by_reasoning: &'a mut BTreeMap<String, MetricCounts>,
    by_surface: &'a mut BTreeMap<String, MetricCounts>,
    by_size: &'a mut BTreeMap<String, MetricCounts>,
    by_category: &'a mut BTreeMap<String, MetricCounts>,
    by_topology: &'a mut BTreeMap<String, MetricCounts>,
    cell: &super::PlanCell,
    task: &super::PublicCorpusTask,
) -> [&'a mut MetricCounts; 6] {
    [
        by_model.get_mut(&cell.model).expect("model dimension"),
        by_reasoning
            .get_mut(&cell.reasoning_level)
            .expect("reasoning dimension"),
        by_surface
            .get_mut(cell.surface.directory())
            .expect("surface dimension"),
        by_size
            .get_mut(&task.difficulty.size_bucket)
            .expect("size dimension"),
        by_category
            .get_mut(&task.category)
            .expect("category dimension"),
        by_topology
            .get_mut(&task.difficulty.topology)
            .expect("topology dimension"),
    ]
}

fn add_initial_grade(counts: &mut MetricCounts, grade: &GradeRecord) {
    counts.harness_failures += usize::from(grade.harness_error);
    counts.strict_schema_successes += usize::from(grade.strict_schema_success);
    counts.local_compile_successes += usize::from(grade.local_compile_success);
    counts.exact_intent_successes += usize::from(grade.exact_intent_success);
    counts.publication_successes += usize::from(grade.publication_success);
    counts.portable_execution_successes += usize::from(grade.portable_execution_success);
    counts.native_execution_successes += usize::from(grade.native_execution_success == Some(true));
}

fn collect_provider_metrics(
    provider: &ProviderMetadata,
    latencies: &mut Vec<u64>,
    input_tokens: &mut u64,
    output_tokens: &mut u64,
    has_input_tokens: &mut bool,
    has_output_tokens: &mut bool,
) {
    if let Some(latency) = provider.latency_ms {
        latencies.push(latency);
    }
    if let Some(tokens) = provider.input_tokens() {
        *input_tokens = input_tokens.saturating_add(tokens);
        *has_input_tokens = true;
    }
    if let Some(tokens) = provider.output_tokens() {
        *output_tokens = output_tokens.saturating_add(tokens);
        *has_output_tokens = true;
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_grade_metrics(
    grade: &GradeRecord,
    taxonomy: &mut BTreeMap<String, usize>,
    paths: &mut BTreeMap<String, usize>,
    authored_bytes: &mut u64,
    authored_operations: &mut u64,
    expanded_operations: &mut u64,
    staged_ratios: &mut Vec<u64>,
) {
    *authored_bytes = authored_bytes.saturating_add(grade.authored_json_bytes.unwrap_or(0) as u64);
    *authored_operations =
        authored_operations.saturating_add(grade.authored_operation_count.unwrap_or(0) as u64);
    *expanded_operations = expanded_operations
        .saturating_add(grade.expanded_graph_operation_count.unwrap_or(0) as u64);
    if let Some(ratio) = grade.staged_compression_ratio_micros {
        staged_ratios.push(ratio);
    }
    if let Some(diagnostic) = &grade.diagnostic {
        *taxonomy.entry(diagnostic.taxonomy.clone()).or_insert(0) += 1;
        *paths.entry(diagnostic.path.clone()).or_insert(0) += 1;
    }
}

fn rate_micros(numerator: usize, denominator: usize) -> Option<u64> {
    (denominator != 0).then(|| ratio_micros(numerator, denominator))
}

fn percentile(sorted: &[u64], percentile: usize) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (percentile * sorted.len()).div_ceil(100).saturating_sub(1);
    sorted.get(rank).copied()
}

pub(crate) fn write_summary(output: &Path, summary: &Summary) -> AnyResult<()> {
    atomic_json(&output.join("summary.json"), summary)?;
    let counts = &summary.counts;
    let surface_rows =
        summary
            .by_surface
            .iter()
            .fold(String::new(), |mut output, (surface, metrics)| {
                writeln!(
                    output,
                    "| {surface} | {} | {} | {} | {} |",
                    metrics.model_correctness_denominator,
                    metrics.strict_schema_successes,
                    metrics.exact_intent_successes,
                    metrics.final_successes
                )
                .expect("write to string");
                output
            });
    let taxonomy_rows = if summary.error_taxonomy.is_empty() {
        "- None recorded.\n".to_owned()
    } else {
        summary
            .error_taxonomy
            .iter()
            .fold(String::new(), |mut output, (taxonomy, count)| {
                writeln!(output, "- {taxonomy}: {count}").expect("write to string");
                output
            })
    };
    let markdown = format!(
        "# AgentIR authoring evaluation\n\nStatus: {}.\n\nThis is an evaluation of bounded large elementwise components, not all AgentIR programs.\n\n## Plan\n\n- Corpus hash: {}\n- Planned initial calls: {}\n- Planned maximum including one repair: {}\n\n## Results\n\n- Attempted cells: {}\n- Completed initial calls: {}\n- Provider failures: {}\n- Harness failures: {}\n- Initial model-correctness denominator: {}\n- Final model-correctness denominator: {}\n- Provider-inclusive denominator: {}\n- Strict schema successes: {}\n- Local compile successes: {}\n- Exact intent successes: {}\n- Publication successes: {}\n- Portable execution successes: {}\n- Native execution successes: {}\n- Repair attempts/recoveries: {}/{}\n- Final successes: {}\n\n## A/B/C comparison\n\n| Surface | Correctness denominator | Strict schema | Exact initial intent | Final success |\n| --- | ---: | ---: | ---: | ---: |\n{}\n## First-error taxonomy\n\n{}\nNo statistical-significance claim is made by this report.\n",
        summary.experiment_status,
        summary.corpus_hash,
        summary.planned_initial_calls,
        summary.planned_maximum_total_calls,
        counts.attempted_cells,
        counts.completed_initial_calls,
        counts.provider_failures,
        counts.harness_failures,
        counts.model_correctness_denominator,
        counts.final_correctness_denominator,
        counts.provider_inclusive_denominator,
        counts.strict_schema_successes,
        counts.local_compile_successes,
        counts.exact_intent_successes,
        counts.publication_successes,
        counts.portable_execution_successes,
        counts.native_execution_successes,
        counts.repair_attempts,
        counts.repair_recoveries,
        counts.final_successes,
        surface_rows,
        taxonomy_rows,
    );
    atomic_write(&output.join("summary.md"), markdown.as_bytes())
}

pub(crate) fn write_reproduction(
    output: &Path,
    configuration: &Configuration,
    plan: &ExecutionPlan,
) -> AnyResult<()> {
    let markdown = format!(
        "# Reproduction\n\nAll commands run from the repository root. No browser or network is needed for generation, comparison, or replay.\n\n    cargo run -p agentir-authoring --bin agentir-authoring-eval -- generate --output {}\n    cargo run -p agentir-authoring --bin agentir-authoring-eval -- replay --output {}\n\nGenerate a second directory and compare semantic artifacts byte-for-byte:\n\n    cargo run -p agentir-authoring --bin agentir-authoring-eval -- generate --output target/authoring-eval/repro-2\n    cargo run -p agentir-authoring --bin agentir-authoring-eval -- compare --left {} --right target/authoring-eval/repro-2\n\nExternal execution is disabled while model or reasoning placeholders remain. Configure comma-separated AGENTIR_AUTHORING_EVAL_MODELS, AGENTIR_AUTHORING_EVAL_REASONING_LEVELS, and a local JSON runner executable in AGENTIR_AUTHORING_EVAL_RUNNER, regenerate, then run:\n\n    cargo run -p agentir-authoring --bin agentir-authoring-eval -- run --output {} --runner /absolute/path/to/runner --parallel 8\n\nThe runner receives one JSON request on stdin and must emit one JSON envelope with status, raw response, and optional latency_ms, usage.input_tokens, usage.output_tokens, and provider_request_id. The stable session_id is fresh per initial cell and reused for its repair.\n\nPlanned initial calls: {}. Conditional repair capacity: {}. Enforced maximum total calls: {}. Native checks requested: {}.\n",
        output.display(),
        output.display(),
        output.display(),
        output.display(),
        plan.planned_initial_calls,
        plan.planned_conditional_repair_calls,
        plan.planned_maximum_total_calls,
        configuration.native,
    );
    atomic_write(&output.join("reproduction.md"), markdown.as_bytes())
}
