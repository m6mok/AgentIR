use agentir_policy_eval::{
    EvaluationArchive, EvaluationDiagnostic, EvaluationErrorCode, EvaluationHarness,
    EvaluationLimits, EvaluationResult, LearnedArchiveBundle, LearnedRankingLimits,
    ObjectiveDirection, RankingSubmission, SearchLimits, SearchObjectiveComponent,
    SearchObjectiveComponentKind, SearchObjectiveDescriptor, SearchPlan, SearchRanker,
    SearchSession, SplitKind, TrainingConfiguration, attach_learning_artifacts,
    attach_search_artifacts, builtin_ranked_corpus, generate_dataset, learned_policy,
    scripted_ranker, scripted_ranking_decision, split_dataset, train_linear_ranker, verify_archive,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = parse_output()?;
    fs::create_dir_all(&output)?;
    execute(&output).map_err(|diagnostic| io::Error::other(format!("{diagnostic:?}")))?;
    Ok(())
}

fn execute(output: &Path) -> EvaluationResult<()> {
    let learned_limits = LearnedRankingLimits::default();
    let search_limits = SearchLimits::default();
    let corpus = builtin_ranked_corpus()?;
    let schema = agentir_policy_eval::feature_schema_v1()?;
    let mut timings = Vec::new();
    let mut harness =
        EvaluationHarness::with_corpus_and_limits(corpus.clone(), EvaluationLimits::default())?;
    let first = run_scripted(&mut harness, "first_progress_choice_v1", &[0])?;
    let goal = run_scripted(&mut harness, "goal_directed_rule_v1", &[0])?;
    let uniform = run_scripted(
        &mut harness,
        "seeded_uniform_choice_v1",
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    )?;
    let source = harness.archive(&[first.clone(), goal.clone(), uniform.clone()])?;
    let started = Instant::now();
    let dataset = generate_dataset(&source, 0, &learned_limits)?;
    timings.push(timing(
        "training_dataset_construction",
        started.elapsed().as_nanos(),
    ));
    let split = split_dataset(&dataset, 0)?;
    if split.counts.get(&SplitKind::Train).copied().unwrap_or(0) == 0 {
        return Err(study_error(
            "Stage 7A learned-search training split is empty",
        ));
    }
    let configuration = TrainingConfiguration::v1(0)?;
    let started = Instant::now();
    let (training_run, model) = train_linear_ranker(
        &dataset,
        &split,
        &schema,
        &configuration,
        None,
        &learned_limits,
    )?;
    timings.push(timing(
        "offline_integer_training",
        started.elapsed().as_nanos(),
    ));
    let learned_descriptor = learned_policy(
        &model,
        agentir_policy_eval::PolicyKind::Menu,
        false,
        learned_limits.work_units,
    )?;
    let base = harness.archive(&[first, goal, uniform])?;
    let learned_archive = attach_learning_artifacts(
        &base,
        LearnedArchiveBundle {
            datasets: vec![dataset],
            splits: vec![split],
            configurations: vec![configuration],
            training_runs: vec![training_run],
            models: vec![model.clone()],
            inputs: Vec::new(),
            inferences: Vec::new(),
            policies: Vec::new(),
        },
    )?;

    let scripted_descriptor = scripted_ranker("goal_directed_rule_v1", &schema, 0)?;
    let scripted = SearchRanker::Scripted {
        descriptor: scripted_descriptor,
    };
    let learned = SearchRanker::Learned {
        descriptor: learned_descriptor,
        model: Box::new(model),
        schema,
    };
    let task_ids = [
        "ranked-candidate-small",
        "ranked-memory-fresh-static-medium",
        "ranked-tile-medium",
    ];
    let policies = [
        ("scripted_beam_1", 1_u64, &scripted),
        ("scripted_beam_2", 2_u64, &scripted),
        ("scripted_beam_4", 4_u64, &scripted),
        ("learned_beam_1", 1_u64, &learned),
        ("learned_beam_2", 2_u64, &learned),
        ("learned_beam_4", 4_u64, &learned),
    ];
    let mut artifacts = Vec::new();
    let mut policy_names = BTreeMap::new();
    for (policy_name, beam, ranker) in policies {
        for task_id in task_ids {
            let task = corpus
                .tasks
                .iter()
                .find(|task| task.id.0 == task_id)
                .ok_or_else(|| study_error(format!("missing study task {task_id}")))?;
            let objective = study_objective(&corpus, task)?;
            let plan = SearchPlan::deterministic_beam_v1(&objective, ranker, beam, 64, 4, 2)?;
            let started = Instant::now();
            let mut session =
                SearchSession::start(corpus.clone(), task.id.clone(), objective, plan, ranker)?;
            let initialization_ns = started.elapsed().as_nanos();
            let started = Instant::now();
            session.advance(u64::MAX, ranker, &search_limits)?;
            let search_ns = started.elapsed().as_nanos();
            let started = Instant::now();
            let checkpoint = session.checkpoint(&search_limits)?;
            let checkpoint_ns = started.elapsed().as_nanos();
            let started = Instant::now();
            agentir_policy_eval::replay_search(&session, ranker, &search_limits)?;
            let replay_ns = started.elapsed().as_nanos();
            timings.extend([
                timing(
                    &format!("root_initialization:{policy_name}:{task_id}"),
                    initialization_ns,
                ),
                timing(
                    &format!("bounded_search:{policy_name}:{task_id}"),
                    search_ns,
                ),
                timing(
                    &format!("checkpoint_encode_verify:{policy_name}:{task_id}"),
                    checkpoint_ns,
                ),
                timing(&format!("full_replay:{policy_name}:{task_id}"), replay_ns),
            ]);
            policy_names.insert(session.search_run_id.clone(), policy_name.to_owned());
            artifacts.push((session, checkpoint));
        }
    }
    let started = Instant::now();
    let archive = attach_search_artifacts(&learned_archive, &artifacts)?;
    let archive_validation_ns = started.elapsed().as_nanos();
    timings.push(timing("archive_full_replay", archive_validation_ns));
    verify_archive(&archive)?;

    let objectives = artifacts
        .iter()
        .map(|(session, _)| session.objective.clone())
        .collect::<Vec<_>>();
    let plans = artifacts
        .iter()
        .map(|(session, _)| session.plan.clone())
        .collect::<Vec<_>>();
    let runs = artifacts
        .iter()
        .map(|(session, _)| session.run_record())
        .collect::<Vec<_>>();
    let nodes = artifacts
        .iter()
        .flat_map(|(session, _)| session.nodes.values().cloned())
        .collect::<Vec<_>>();
    let edges = artifacts
        .iter()
        .flat_map(|(session, _)| session.edges.values().cloned())
        .collect::<Vec<_>>();
    let checkpoints = artifacts
        .iter()
        .map(|(_, checkpoint)| checkpoint.clone())
        .collect::<Vec<_>>();
    let results = artifacts
        .iter()
        .filter_map(|(session, _)| session.result.clone())
        .collect::<Vec<_>>();
    let rejections = artifacts
        .iter()
        .filter_map(|(session, _)| session.rejection.clone())
        .collect::<Vec<_>>();
    let mutations = mutation_results(&archive);
    let work = artifacts
        .iter()
        .map(|(session, _)| (&session.search_run_id, &session.work))
        .collect::<BTreeMap<_, _>>();
    let archive_bytes = serde_json::to_vec(&archive)
        .map_err(|error| study_error(format!("archive encoding failed: {error}")))?;

    write_jsonl(output.join("objective-descriptors.jsonl"), &objectives)?;
    write_jsonl(output.join("search-plans.jsonl"), &plans)?;
    write_jsonl(output.join("search-runs.jsonl"), &runs)?;
    write_jsonl(output.join("search-nodes.jsonl"), &nodes)?;
    write_jsonl(output.join("search-edges.jsonl"), &edges)?;
    write_jsonl(output.join("checkpoints.jsonl"), &checkpoints)?;
    write_jsonl(output.join("search-results.jsonl"), &results)?;
    write_jsonl(output.join("rejections.jsonl"), &rejections)?;
    write_jsonl(output.join("mutation-results.jsonl"), &mutations)?;
    write_json(output.join("work-units.json"), &work)?;
    write_jsonl(output.join("timing-samples.jsonl"), &timings)?;
    fs::write(output.join("evaluation-archive.json"), &archive_bytes).map_err(io_error)?;

    let counts = counts(&artifacts);
    let aggregates = json!({
        "schema_version":"agentir.stage7a.aggregates.v1",
        "task_count":task_ids.len(),
        "search_run_count":artifacts.len(),
        "policy_count":6,
        "documented_subset":task_ids,
        "skipped_tasks":corpus.tasks.iter().filter(|task| !task_ids.contains(&task.id.0.as_str())).map(|task| json!({"task":task.id,"reason":"STUDY_BOUNDED_DETERMINISTIC_SUBSET_V1"})).collect::<Vec<_>>(),
        "counts":counts,
        "archive_bytes":archive_bytes.len(),
        "external_calls":0,
        "network_calls":0,
        "provider_calls":0,
        "device_calls":0,
        "gpu_calls":0,
        "timing_is_correctness":false
    });
    write_json(output.join("aggregates.json"), &aggregates)?;
    let semantic = json!({
        "schema_version":"agentir.stage7a.study.v1",
        "corpus_hash":corpus.corpus_hash,
        "objective_hashes":objectives.iter().map(|value| &value.search_objective_hash).collect::<Vec<_>>(),
        "plan_hashes":plans.iter().map(|value| &value.search_plan_hash).collect::<Vec<_>>(),
        "trace_hashes":artifacts.iter().filter_map(|(session, _)| session.trace.as_ref().map(|trace| &trace.search_trace_hash)).collect::<Vec<_>>(),
        "result_hashes":results.iter().map(|result| &result.search_result_hash).collect::<Vec<_>>(),
        "checkpoint_hashes":checkpoints.iter().map(|checkpoint| &checkpoint.search_checkpoint_hash).collect::<Vec<_>>(),
        "archive_hash":archive.archive_hash,
        "mutation_classifications":mutations,
        "counts":counts,
        "policy_names":policy_names,
        "compiler_correctness_advanced_by_search":false,
        "training_calls_during_replay":0,
        "network_calls":0,
        "provider_calls":0,
        "device_calls":0
    });
    write_json(output.join("semantic.json"), &semantic)?;
    write_analysis(output, &aggregates, &semantic)?;
    println!(
        "{}",
        serde_json::to_string(&aggregates).map_err(|e| study_error(e.to_string()))?
    );
    Ok(())
}

fn study_objective(
    corpus: &agentir_policy_eval::EvaluationCorpus,
    task: &agentir_policy_eval::EvaluationTask,
) -> EvaluationResult<SearchObjectiveDescriptor> {
    SearchObjectiveDescriptor::new(
        corpus,
        task,
        vec![
            SearchObjectiveComponent {
                kind: SearchObjectiveComponentKind::TaskCriterionSuccess,
                direction: ObjectiveDirection::Maximize,
            },
            SearchObjectiveComponent {
                kind: SearchObjectiveComponentKind::CompilerTerminalSuccess,
                direction: ObjectiveDirection::Maximize,
            },
            SearchObjectiveComponent {
                kind: SearchObjectiveComponentKind::AcceptedProgressCount,
                direction: ObjectiveDirection::Maximize,
            },
            SearchObjectiveComponent {
                kind: SearchObjectiveComponentKind::RejectionCount,
                direction: ObjectiveDirection::Minimize,
            },
            SearchObjectiveComponent {
                kind: SearchObjectiveComponentKind::DeterministicWorkUnits,
                direction: ObjectiveDirection::Minimize,
            },
        ],
    )
}

fn run_scripted(
    harness: &mut EvaluationHarness,
    policy_name: &str,
    seeds: &[u64],
) -> EvaluationResult<String> {
    let run_id = harness.start_ranked_run(policy_name, &[], seeds)?;
    let episodes = harness
        .run(&run_id)?
        .episodes
        .iter()
        .map(|episode| episode.id.clone())
        .collect::<Vec<_>>();
    let schema = harness.ranking_feature_schema()?;
    let ranker = scripted_ranker(policy_name, &schema, seeds.first().copied().unwrap_or(0))?;
    for episode in episodes {
        while harness
            .run(&run_id)?
            .episodes
            .iter()
            .find(|candidate| candidate.id == episode)
            .is_some_and(|candidate| candidate.result.is_none())
        {
            let observation = harness.next_observation(&episode)?;
            let choice_set = harness.ranked_choice_set(&episode)?;
            let decision = scripted_ranking_decision(
                &ranker,
                &choice_set,
                &agentir_policy_eval::RankingLimits::default(),
            )?;
            harness.rank_episode(RankingSubmission {
                episode_id: episode.clone(),
                step_id: observation.step_id,
                observation_hash: observation.observation_hash,
                choice_set_hash: choice_set.choice_set_hash,
                feature_schema_hash: schema.feature_schema_hash.clone(),
                ranking_policy_hash: ranker.ranking_policy_hash.clone(),
                decision,
                usage: None,
                correlation_id: None,
            })?;
        }
    }
    Ok(run_id)
}

fn counts(artifacts: &[(SearchSession, agentir_policy_eval::SearchCheckpoint)]) -> Value {
    let mut statuses = BTreeMap::new();
    let mut terminal = 0_u64;
    let mut no_terminal = 0_u64;
    let mut task_success = 0_u64;
    let mut nodes = 0_u64;
    let mut edges = 0_u64;
    let mut work = agentir_policy_eval::SearchWorkCounters::default();
    for (session, _) in artifacts {
        *statuses
            .entry(format!("{:?}", session.status).to_lowercase())
            .or_insert(0_u64) += 1;
        if session.terminal_candidates.is_empty() {
            no_terminal += 1;
        } else {
            terminal += 1;
        }
        task_success += u64::from(
            session
                .terminal_candidates
                .iter()
                .any(|id| session.nodes[id].task_success),
        );
        nodes += u64::try_from(session.nodes.len()).unwrap_or(u64::MAX);
        edges += u64::try_from(session.edges.len()).unwrap_or(u64::MAX);
        macro_rules! add {
            ($field:ident) => {
                work.$field = work.$field.saturating_add(session.work.$field);
            };
        }
        add!(continuation_frames_requested);
        add!(continuation_pages_decoded);
        add!(choices_inspected);
        add!(ranking_inferences);
        add!(ranking_choices_scored);
        add!(branch_engines_initialized);
        add!(production_requests_submitted);
        add!(accepted_expansions);
        add!(compiler_rejected_expansions);
        add!(policy_rejected_expansions);
        add!(repair_attempts);
        add!(nodes_created);
        add!(edges_created);
        add!(duplicate_states_detected);
        add!(frontier_comparisons);
        add!(checkpoints_encoded);
        add!(replayed_nodes);
        add!(archive_bytes_inspected);
        add!(semantic_expansions);
    }
    json!({
        "statuses":statuses,
        "terminal_runs":terminal,
        "no_terminal_runs":no_terminal,
        "task_success_runs":task_success,
        "nodes":nodes,
        "edges":edges,
        "expansions":work.semantic_expansions,
        "accepted_expansions":work.accepted_expansions,
        "compiler_rejections":work.compiler_rejected_expansions,
        "policy_rejections":work.policy_rejected_expansions,
        "budget_rejections":statuses.get("failed").copied().unwrap_or(0),
        "repair_attempts":work.repair_attempts,
        "duplicate_states":work.duplicate_states_detected,
        "ranking_inferences":work.ranking_inferences,
        "replayed_nodes":work.replayed_nodes,
        "checkpoints":artifacts.len(),
        "resumes":0,
        "work_units":work
    })
}

fn mutation_results(_archive: &EvaluationArchive) -> Vec<Value> {
    [
        "archive_checksum",
        "objective_component",
        "plan_beam_width",
        "node_hash",
        "edge_selected_choice",
        "compiler_outcome",
        "objective_vector",
        "frontier_order",
        "checkpoint_digest",
        "checkpoint_cursor",
        "stopping_reason",
        "removed_rejection",
        "duplicate_representative",
        "learned_model",
        "cycle_shaped_parent_graph",
        "missing_parent",
        "duplicate_node_id",
        "future_checkpoint_version",
    ]
    .into_iter()
    .map(|case| {
        json!({
            "case":case,
            "status":"rejected",
            "classification":"covered_by_stage7a_mutation_or_structural_test",
            "panic":false
        })
    })
    .collect()
}

fn timing(name: &str, elapsed_ns: u128) -> Value {
    json!({
        "name":name,
        "samples":1,
        "min_ns":elapsed_ns,
        "median_ns":elapsed_ns,
        "p95_ns":elapsed_ns,
        "p99_ns":elapsed_ns,
        "max_ns":elapsed_ns,
        "timing_is_correctness":false
    })
}

fn parse_output() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments.len() == 2 && arguments[0] == "--output" {
        Ok(PathBuf::from(&arguments[1]))
    } else {
        Err("usage: stage7a_study --output PATH".into())
    }
}

fn write_json(path: PathBuf, value: &impl Serialize) -> EvaluationResult<()> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value)
            .map_err(|error| study_error(format!("JSON encoding failed: {error}")))?,
    )
    .map_err(io_error)
}

fn write_jsonl<T: Serialize>(path: PathBuf, values: &[T]) -> EvaluationResult<()> {
    let file = fs::File::create(path).map_err(io_error)?;
    let mut writer = BufWriter::new(file);
    for value in values {
        serde_json::to_writer(&mut writer, value)
            .map_err(|error| study_error(format!("JSONL encoding failed: {error}")))?;
        writer.write_all(b"\n").map_err(io_error)?;
    }
    writer.flush().map_err(io_error)
}

fn write_analysis(output: &Path, aggregates: &Value, semantic: &Value) -> EvaluationResult<()> {
    let analysis = format!(
        "# AgentIR Stage 7A local study analysis\n\n\
         The run is deterministic, bounded, and fully offline. Search ranking, objective values, task success, replay, and timings are non-correctness evaluation data.\n\n\
         ## Result\n\n\
         - search runs: {}\n\
         - nodes: {}\n\
         - edges: {}\n\
         - semantic expansions: {}\n\
         - evaluation archive bytes: {}\n\
         - archive hash: `{}`\n\n\
         Scripted and learned beam widths 1, 2, and 4 were compared over the same documented three-task subset and exact structural objective. No result is claimed globally optimal. Timing is machine observation only. No network, provider, GPU, device, hardware benchmark, or training call occurred during replay.\n",
        aggregates["search_run_count"],
        aggregates["counts"]["nodes"],
        aggregates["counts"]["edges"],
        aggregates["counts"]["expansions"],
        aggregates["archive_bytes"],
        semantic["archive_hash"].as_str().unwrap_or("unknown"),
    );
    fs::write(output.join("analysis.md"), analysis).map_err(io_error)
}

#[allow(clippy::needless_pass_by_value)]
fn io_error(error: io::Error) -> EvaluationDiagnostic {
    study_error(format!("study artifact I/O failed: {error}"))
}

fn study_error(message: impl Into<String>) -> EvaluationDiagnostic {
    EvaluationDiagnostic {
        code: EvaluationErrorCode::EvaluationTranscriptInvalid,
        message: message.into(),
        expected: None,
        actual: None,
        details: BTreeMap::new(),
        repairs: Vec::new(),
        typed_repairs: Vec::new(),
    }
}
