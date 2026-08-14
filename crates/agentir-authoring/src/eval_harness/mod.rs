use agentir_authoring::{
    AuthoringSurface, AuthoringTask, GraphProposal, IncrementalBatch, StagedProposal,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

mod corpus;
mod grader;
mod report;
mod runner;
mod v2;

pub(crate) const FORMAT_VERSION: u32 = 1;
pub(crate) const CORPUS_SEED: u64 = 20_260_813;
pub(crate) const TASK_COUNT: usize = 30;
pub(crate) const REQUESTED_TRIALS: usize = 5;
pub(crate) const MAX_INITIAL_CALLS: usize = 900;
pub(crate) const MAX_REPAIRS_PER_ATTEMPT: usize = 1;
pub(crate) const MAX_TOTAL_CALLS: usize = 1_800;
pub(crate) const MAX_PARALLEL_CALLS: usize = 8;
pub(crate) const MAX_PROVIDER_RESPONSE_BYTES: usize = 262_144;
const DEFAULT_MODELS: [&str; 3] = ["<MODEL_1>", "<MODEL_2>", "<MODEL_3>"];
const DEFAULT_REASONING_LEVELS: [&str; 2] = ["<LEVEL_1>", "<LEVEL_2>"];
pub(crate) const SURFACES: [SurfaceName; 3] = [
    SurfaceName::Graph,
    SurfaceName::IncrementalBatch,
    SurfaceName::Staged,
];

pub(crate) type AnyError = Box<dyn Error + Send + Sync>;
pub(crate) type AnyResult<T> = Result<T, AnyError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SurfaceName {
    Graph,
    IncrementalBatch,
    Staged,
}

impl SurfaceName {
    pub(crate) const fn sdk(self) -> AuthoringSurface {
        match self {
            Self::Graph => AuthoringSurface::Graph,
            Self::IncrementalBatch => AuthoringSurface::IncrementalBatch,
            Self::Staged => AuthoringSurface::Staged,
        }
    }

    pub(crate) const fn directory(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::IncrementalBatch => "incremental-batch",
            Self::Staged => "staged",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Configuration {
    pub(crate) models: Vec<String>,
    pub(crate) reasoning_levels: Vec<String>,
    pub(crate) native: bool,
}

impl Configuration {
    fn from_environment() -> Self {
        Self {
            models: configured_list("AGENTIR_AUTHORING_EVAL_MODELS", &DEFAULT_MODELS),
            reasoning_levels: configured_list(
                "AGENTIR_AUTHORING_EVAL_REASONING_LEVELS",
                &DEFAULT_REASONING_LEVELS,
            ),
            native: std::env::var_os("AGENTIR_AUTHORING_EVAL_PORTABLE_ONLY").is_none(),
        }
    }

    pub(crate) fn placeholders(&self) -> bool {
        self.models
            .iter()
            .chain(&self.reasoning_levels)
            .any(|value| value.starts_with('<') && value.ends_with('>'))
    }
}

fn configured_list(name: &str, defaults: &[&str]) -> Vec<String> {
    std::env::var(name).map_or_else(
        |_| defaults.iter().map(|value| (*value).to_owned()).collect(),
        |value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        },
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Difficulty {
    pub(crate) size_bucket: String,
    pub(crate) topology: String,
    pub(crate) expanded_operations: usize,
    pub(crate) body_operations: usize,
    pub(crate) stages: usize,
    pub(crate) recurrence_lags: Vec<usize>,
    pub(crate) warmup_lengths: Vec<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DependencyStatistics {
    pub(crate) local_references: usize,
    pub(crate) maximum_reference_distance: usize,
    pub(crate) maximum_fan_out: usize,
    pub(crate) reused_local_values: usize,
    pub(crate) repeated_operand_operations: usize,
    pub(crate) fma_operations: usize,
    pub(crate) non_final_yield: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RepresentationMetadata {
    pub(crate) graph_authored_operations: usize,
    pub(crate) incremental_authored_operations: usize,
    pub(crate) incremental_transactions: usize,
    pub(crate) incremental_max_transaction_operations: usize,
    pub(crate) staged_authored_operations: usize,
    pub(crate) expanded_graph_operations: usize,
    pub(crate) staged_compression_ratio_micros: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PublicCorpusTask {
    pub(crate) task_id: String,
    pub(crate) category: String,
    pub(crate) difficulty: Difficulty,
    pub(crate) public_specification: String,
    pub(crate) scalars: Vec<String>,
    pub(crate) tensors: Vec<String>,
    pub(crate) expected_operation_count: usize,
    pub(crate) dependency_statistics: DependencyStatistics,
    pub(crate) representations: RepresentationMetadata,
    pub(crate) paired_surfaces: Vec<SurfaceName>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PrivateCorpusTask {
    pub(crate) public: PublicCorpusTask,
    pub(crate) server_task: AuthoringTask,
    pub(crate) graph_payload: GraphProposal,
    pub(crate) incremental_payload: IncrementalBatch,
    pub(crate) staged_payload: StagedProposal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PublicCorpus {
    format: String,
    format_version: u32,
    seed: u64,
    task_count: usize,
    pub(crate) corpus_hash: String,
    pub(crate) tasks: Vec<PublicCorpusTask>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PrivateCorpus {
    format: String,
    format_version: u32,
    seed: u64,
    pub(crate) corpus_hash: String,
    pub(crate) tasks: Vec<PrivateCorpusTask>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlanCell {
    pub(crate) cell_id: String,
    pub(crate) model: String,
    pub(crate) reasoning_level: String,
    pub(crate) task_id: String,
    pub(crate) surface: SurfaceName,
    pub(crate) trial_index: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ExecutionPlan {
    format: String,
    format_version: u32,
    pub(crate) corpus_hash: String,
    requested_trials_per_cell: usize,
    pub(crate) selected_trials_per_cell: usize,
    trial_reduction_reason: Option<String>,
    pub(crate) planned_initial_calls: usize,
    pub(crate) planned_conditional_repair_calls: usize,
    pub(crate) planned_maximum_total_calls: usize,
    max_initial_model_calls: usize,
    max_repairs_per_attempt: usize,
    max_total_model_calls: usize,
    max_parallel_calls: usize,
    pub(crate) cells: Vec<PlanCell>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
struct ExperimentRecord {
    format: String,
    format_version: u32,
    corpus_seed: u64,
    task_count: usize,
    models: Vec<String>,
    reasoning_levels: Vec<String>,
    requested_trials_per_cell: usize,
    selected_trials_per_cell: usize,
    limits: Value,
    output_directory: String,
    commit: String,
    dirty_status: Vec<String>,
    generated_unix_seconds: u64,
    environment: BTreeMap<String, String>,
    runner_configured: bool,
    model_configuration_complete: bool,
    external_calls_executed: bool,
    native_checks_requested: bool,
    prompt_oracle_audit_passed: bool,
}

#[derive(Clone, Debug)]
struct Arguments {
    command: String,
    output: Option<PathBuf>,
    left: Option<PathBuf>,
    right: Option<PathBuf>,
    runner: Option<PathBuf>,
    parallel: usize,
}

fn parse_arguments() -> AnyResult<Arguments> {
    let mut values = std::env::args_os().skip(1);
    let command = values
        .next()
        .ok_or("missing command: generate, generate-v2, run, replay, verify-replay, or compare")?
        .into_string()
        .map_err(|_| "command is not UTF-8")?;
    let mut arguments = Arguments {
        command,
        output: None,
        left: None,
        right: None,
        runner: std::env::var_os("AGENTIR_AUTHORING_EVAL_RUNNER").map(PathBuf::from),
        parallel: MAX_PARALLEL_CALLS,
    };
    while let Some(flag) = values.next() {
        let value = values.next().ok_or("option requires a value")?;
        match flag.to_str() {
            Some("--output") => arguments.output = Some(PathBuf::from(value)),
            Some("--left") => arguments.left = Some(PathBuf::from(value)),
            Some("--right") => arguments.right = Some(PathBuf::from(value)),
            Some("--runner") => arguments.runner = Some(PathBuf::from(value)),
            Some("--parallel") => {
                arguments.parallel = value
                    .to_str()
                    .ok_or("parallel value is not UTF-8")?
                    .parse()?;
            }
            _ => return Err("unknown option".into()),
        }
    }
    if arguments.parallel == 0 || arguments.parallel > MAX_PARALLEL_CALLS {
        return Err(format!("parallel must be between 1 and {MAX_PARALLEL_CALLS}").into());
    }
    Ok(arguments)
}

pub(crate) fn run_cli() -> AnyResult<()> {
    let arguments = parse_arguments()?;
    let configuration = Configuration::from_environment();
    match arguments.command.as_str() {
        "generate" => generate(
            arguments.output.as_deref().ok_or("--output is required")?,
            &configuration,
            arguments.runner.as_deref(),
        ),
        "generate-v2" => v2::generate_v2(
            arguments.output.as_deref().ok_or("--output is required")?,
            &configuration,
        ),
        "run" => runner::run_experiment(
            arguments.output.as_deref().ok_or("--output is required")?,
            &configuration,
            arguments.runner.as_deref().ok_or("--runner is required")?,
            arguments.parallel,
        ),
        "replay" => runner::replay(
            arguments.output.as_deref().ok_or("--output is required")?,
            configuration.native,
        ),
        "verify-replay" => runner::verify_replay(
            arguments.output.as_deref().ok_or("--output is required")?,
            configuration.native,
        ),
        "compare" => compare(
            arguments.left.as_deref().ok_or("--left is required")?,
            arguments.right.as_deref().ok_or("--right is required")?,
        ),
        _ => Err(
            "command must be generate, generate-v2, run, replay, verify-replay, or compare".into(),
        ),
    }
}

fn generate(output: &Path, configuration: &Configuration, runner: Option<&Path>) -> AnyResult<()> {
    fs::create_dir_all(output)?;
    let tasks = corpus::build_corpus()?;
    let corpus_hash = corpus::corpus_hash(&tasks)?;
    let public = PublicCorpus {
        format: "agentir.authoring_eval.corpus.public".to_owned(),
        format_version: FORMAT_VERSION,
        seed: CORPUS_SEED,
        task_count: tasks.len(),
        corpus_hash: corpus_hash.clone(),
        tasks: tasks.iter().map(|task| task.public.clone()).collect(),
    };
    let private = PrivateCorpus {
        format: "agentir.authoring_eval.corpus.private".to_owned(),
        format_version: FORMAT_VERSION,
        seed: CORPUS_SEED,
        corpus_hash: corpus_hash.clone(),
        tasks,
    };
    let plan = build_plan(configuration, &corpus_hash, &private.tasks)?;
    corpus::audit_prompts(&private.tasks)?;
    atomic_json(&output.join("corpus.json"), &public)?;
    atomic_json(&output.join("corpus-private.json"), &private)?;
    atomic_write(
        &output.join("corpus-hash.txt"),
        format!("{corpus_hash}\n").as_bytes(),
    )?;
    atomic_json(&output.join("execution-plan.json"), &plan)?;
    corpus::write_prompts(output, &private.tasks)?;
    let experiment = experiment_record(
        output,
        configuration,
        runner.is_some(),
        false,
        plan.selected_trials_per_cell,
    );
    atomic_json(&output.join("experiment.json"), &experiment)?;
    let summary = report::build_summary(output, &public, &plan, "dry_run")?;
    report::write_summary(output, &summary)?;
    report::write_reproduction(output, configuration, &plan)?;
    append_event(
        output,
        &serde_json::json!({
            "event":"generated",
            "corpus_hash":corpus_hash,
            "planned_initial_calls":plan.planned_initial_calls,
            "planned_maximum_total_calls":plan.planned_maximum_total_calls
        }),
    )?;
    println!(
        "generated {} tasks; planned initial calls: {}; maximum with repairs: {}",
        private.tasks.len(),
        plan.planned_initial_calls,
        plan.planned_maximum_total_calls
    );
    Ok(())
}

fn build_plan(
    configuration: &Configuration,
    corpus_hash: &str,
    tasks: &[PrivateCorpusTask],
) -> AnyResult<ExecutionPlan> {
    if configuration.models.is_empty() || configuration.reasoning_levels.is_empty() {
        return Err("models and reasoning levels must be non-empty".into());
    }
    let cells_per_trial = configuration
        .models
        .len()
        .checked_mul(configuration.reasoning_levels.len())
        .and_then(|count| count.checked_mul(tasks.len()))
        .and_then(|count| count.checked_mul(SURFACES.len()))
        .ok_or("plan size overflow")?;
    let selected_trials = REQUESTED_TRIALS
        .min(MAX_INITIAL_CALLS / cells_per_trial)
        .min(MAX_TOTAL_CALLS / cells_per_trial / (1 + MAX_REPAIRS_PER_ATTEMPT));
    if selected_trials == 0 {
        return Err("call budgets cannot cover one paired trial".into());
    }
    let mut cells = Vec::with_capacity(cells_per_trial * selected_trials);
    for model in &configuration.models {
        for reasoning_level in &configuration.reasoning_levels {
            for task in tasks {
                for surface in SURFACES {
                    for trial_index in 0..selected_trials {
                        cells.push(PlanCell {
                            cell_id: format!(
                                "{}__{}__{}__{}__{}",
                                safe_component(model),
                                safe_component(reasoning_level),
                                task.public.task_id,
                                surface.directory(),
                                trial_index
                            ),
                            model: model.clone(),
                            reasoning_level: reasoning_level.clone(),
                            task_id: task.public.task_id.clone(),
                            surface,
                            trial_index,
                        });
                    }
                }
            }
        }
    }
    let initial = cells.len();
    let repairs = initial * MAX_REPAIRS_PER_ATTEMPT;
    let total = initial + repairs;
    if initial > MAX_INITIAL_CALLS || total > MAX_TOTAL_CALLS {
        return Err("constructed execution plan exceeds model-call limits".into());
    }
    Ok(ExecutionPlan {
        format: "agentir.authoring_eval.execution_plan".to_owned(),
        format_version: FORMAT_VERSION,
        corpus_hash: corpus_hash.to_owned(),
        requested_trials_per_cell: REQUESTED_TRIALS,
        selected_trials_per_cell: selected_trials,
        trial_reduction_reason: (selected_trials < REQUESTED_TRIALS).then(|| {
            format!(
                "deterministically reduced from {REQUESTED_TRIALS} to {selected_trials} while preserving every task/surface/model/reasoning cell"
            )
        }),
        planned_initial_calls: initial,
        planned_conditional_repair_calls: repairs,
        planned_maximum_total_calls: total,
        max_initial_model_calls: MAX_INITIAL_CALLS,
        max_repairs_per_attempt: MAX_REPAIRS_PER_ATTEMPT,
        max_total_model_calls: MAX_TOTAL_CALLS,
        max_parallel_calls: MAX_PARALLEL_CALLS,
        cells,
    })
}

pub(crate) fn safe_component(value: &str) -> String {
    let normalized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if normalized.is_empty() {
        "unnamed".to_owned()
    } else {
        normalized
    }
}

fn experiment_record(
    output: &Path,
    configuration: &Configuration,
    runner_configured: bool,
    calls_executed: bool,
    selected_trials: usize,
) -> ExperimentRecord {
    let commit =
        command_output("git", &["rev-parse", "HEAD"]).unwrap_or_else(|_| "UNKNOWN".to_owned());
    let dirty_status = command_output("git", &["status", "--short"])
        .unwrap_or_else(|error| format!("UNAVAILABLE: {error}"))
        .lines()
        .map(str::to_owned)
        .collect();
    let mut environment = BTreeMap::new();
    environment.insert("os".to_owned(), std::env::consts::OS.to_owned());
    environment.insert("arch".to_owned(), std::env::consts::ARCH.to_owned());
    environment.insert(
        "rustc".to_owned(),
        command_output("rustc", &["--version"]).unwrap_or_else(|_| "UNKNOWN".to_owned()),
    );
    ExperimentRecord {
        format: "agentir.authoring_eval.experiment".to_owned(),
        format_version: FORMAT_VERSION,
        corpus_seed: CORPUS_SEED,
        task_count: TASK_COUNT,
        models: configuration.models.clone(),
        reasoning_levels: configuration.reasoning_levels.clone(),
        requested_trials_per_cell: REQUESTED_TRIALS,
        selected_trials_per_cell: selected_trials,
        limits: serde_json::json!({
            "max_initial_model_calls":MAX_INITIAL_CALLS,
            "max_repairs_per_attempt":MAX_REPAIRS_PER_ATTEMPT,
            "max_total_model_calls":MAX_TOTAL_CALLS,
            "max_parallel_calls":MAX_PARALLEL_CALLS
        }),
        output_directory: output.display().to_string(),
        commit: commit.trim().to_owned(),
        dirty_status,
        generated_unix_seconds: now_unix_seconds(),
        environment,
        runner_configured,
        model_configuration_complete: !configuration.placeholders(),
        external_calls_executed: calls_executed,
        native_checks_requested: configuration.native,
        prompt_oracle_audit_passed: true,
    }
}

fn command_output(program: &str, arguments: &[&str]) -> AnyResult<String> {
    let output = Command::new(program).args(arguments).output()?;
    if !output.status.success() {
        return Err(format!("{program} exited with {}", output.status).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim_end().to_owned())
}

fn compare(left: &Path, right: &Path) -> AnyResult<()> {
    let left_private: PrivateCorpus = read_json(&left.join("corpus-private.json"))?;
    let right_private: PrivateCorpus = read_json(&right.join("corpus-private.json"))?;
    let left_plan: ExecutionPlan = read_json(&left.join("execution-plan.json"))?;
    let right_plan: ExecutionPlan = read_json(&right.join("execution-plan.json"))?;
    if left_private.corpus_hash != right_private.corpus_hash
        || left_plan.cells != right_plan.cells
        || left_private.tasks.len() != right_private.tasks.len()
    {
        return Err("semantic corpus or execution plan differs".into());
    }
    let mut semantic_files = vec![
        PathBuf::from("corpus.json"),
        PathBuf::from("corpus-private.json"),
        PathBuf::from("corpus-hash.txt"),
        PathBuf::from("execution-plan.json"),
    ];
    for task in &left_private.tasks {
        for surface in SURFACES {
            semantic_files.push(
                PathBuf::from("prompts")
                    .join(&task.public.task_id)
                    .join(format!("{}.txt", surface.directory())),
            );
        }
    }
    for relative in semantic_files {
        if fs::read(left.join(&relative))? != fs::read(right.join(&relative))? {
            return Err(format!("semantic artifact differs: {}", relative.display()).into());
        }
    }
    println!("semantic artifacts are byte-identical");
    Ok(())
}

pub(crate) fn verify_loaded_artifacts(
    public: &PublicCorpus,
    private: &PrivateCorpus,
    plan: &ExecutionPlan,
) -> AnyResult<()> {
    let actual_hash = corpus::corpus_hash(&private.tasks)?;
    if actual_hash != public.corpus_hash
        || actual_hash != private.corpus_hash
        || actual_hash != plan.corpus_hash
    {
        return Err("corpus hash verification failed".into());
    }
    corpus::audit_prompts(&private.tasks)?;
    Ok(())
}

pub(crate) fn attempt_directory(output: &Path, cell: &PlanCell) -> PathBuf {
    output
        .join("attempts")
        .join(safe_component(&cell.model))
        .join(safe_component(&cell.reasoning_level))
        .join(&cell.task_id)
        .join(cell.surface.directory())
        .join(cell.trial_index.to_string())
}

pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> AnyResult<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> AnyResult<()> {
    let parent = path.parent().ok_or("path has no parent")?;
    fs::create_dir_all(parent)?;
    let file_name = path.file_name().ok_or("path has no file name")?;
    let mut temporary_name = OsString::from(".");
    temporary_name.push(file_name);
    temporary_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        now_unix_seconds()
    ));
    let temporary = parent.join(temporary_name);
    let mut file = File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> AnyResult<T> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) fn append_event(output: &Path, event: &Value) -> AnyResult<()> {
    let path = output.join("events.jsonl");
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let bounded = serde_json::json!({
        "format":"agentir.authoring_eval.event.v1",
        "unix_seconds":now_unix_seconds(),
        "record":event,
    });
    let bytes = serde_json::to_vec(&bounded)?;
    if bytes.len() > 16_384 {
        return Err("event exceeds 16 KiB bound".into());
    }
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}

pub(crate) fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub(crate) fn ratio_micros(numerator: usize, denominator: usize) -> u64 {
    if denominator == 0 {
        return 0;
    }
    u64::try_from((numerator as u128 * 1_000_000) / denominator as u128).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_reduces_trials_without_dropping_cells() {
        let configuration = Configuration::from_environment();
        let tasks = corpus::build_corpus().unwrap();
        let hash = corpus::corpus_hash(&tasks).unwrap();
        let plan = build_plan(&configuration, &hash, &tasks).unwrap();
        assert_eq!(plan.selected_trials_per_cell, 1);
        assert_eq!(plan.planned_initial_calls, 540);
        assert_eq!(plan.planned_maximum_total_calls, 1_080);
        assert_eq!(plan.cells.len(), 3 * 2 * 30 * 3);
    }
}
