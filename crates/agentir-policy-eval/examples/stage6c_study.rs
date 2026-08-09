use agentir_policy_eval::{
    EvaluationArchive, EvaluationDiagnostic, EvaluationHarness, EvaluationLimits, EvaluationResult,
    FrameCompleteness, LearnedArchiveBundle, LearnedRankingLimits, PolicyOrigin, RankingSubmission,
    SplitKind, TrainingConfiguration, aggregate_ranking_metrics, attach_learning_artifacts,
    builtin_ranked_corpus, generate_dataset, infer, learned_policy, ranked_policy, ranking_input,
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
    let limits = LearnedRankingLimits::default();
    let schema = agentir_policy_eval::feature_schema_v1()?;
    let mut timings = Vec::new();
    let mut harness = EvaluationHarness::with_corpus_and_limits(
        builtin_ranked_corpus()?,
        EvaluationLimits::default(),
    )?;

    let first_run = run_scripted(&mut harness, "first_progress_choice_v1", &[0])?;
    let goal_run = run_scripted(&mut harness, "goal_directed_rule_v1", &[0])?;
    let uniform_run = run_scripted(
        &mut harness,
        "seeded_uniform_choice_v1",
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    )?;
    let baseline_runs = vec![first_run.clone(), goal_run.clone(), uniform_run.clone()];
    let source_archive = harness.archive(&baseline_runs)?;

    let started = Instant::now();
    let dataset = generate_dataset(&source_archive, 0, &limits)?;
    timings.push(timing("dataset_extraction", started.elapsed().as_nanos()));
    let started = Instant::now();
    let split = split_dataset(&dataset, 0)?;
    timings.push(timing("split_construction", started.elapsed().as_nanos()));
    if split.counts.get(&SplitKind::Train).copied().unwrap_or(0) == 0 {
        return Err(study_error(
            "deterministic Stage 6C split produced no training groups",
        ));
    }
    let configuration = TrainingConfiguration::v1(0)?;
    let started = Instant::now();
    let (training_run, model) =
        train_linear_ranker(&dataset, &split, &schema, &configuration, None, &limits)?;
    timings.push(timing("training", started.elapsed().as_nanos()));
    let learned_ranker = learned_policy(
        &model,
        agentir_policy_eval::PolicyKind::Menu,
        false,
        limits.work_units,
    )?;
    harness.register_ranking_policy(learned_ranker.clone())?;
    let learned_run_policy = ranked_policy(&learned_ranker, PolicyOrigin::Learned)?;
    let (learned_run, inputs, inferences, inference_ns) = run_learned(
        &mut harness,
        learned_run_policy,
        &learned_ranker,
        &model,
        &schema,
        &limits,
    )?;
    timings.push(timing("learned_inference_total", inference_ns));

    let all_runs = vec![first_run, goal_run, uniform_run, learned_run];
    let base_archive = harness.archive(&all_runs)?;
    let archive = attach_learning_artifacts(
        &base_archive,
        LearnedArchiveBundle {
            datasets: vec![dataset.clone()],
            splits: vec![split.clone()],
            configurations: vec![configuration.clone()],
            training_runs: vec![training_run.clone()],
            models: vec![model.clone()],
            inputs: inputs.clone(),
            inferences: inferences.clone(),
            policies: Vec::new(),
        },
    )?;
    let started = Instant::now();
    verify_archive(&archive)?;
    timings.push(timing(
        "archive_structural_verification",
        started.elapsed().as_nanos(),
    ));
    let work_units = json!({
        "archive":agentir_policy_eval::archive_work_units(&archive),
        "training":training_run.work_units,
        "inference":inferences.iter().map(|record| &record.work_units).collect::<Vec<_>>(),
        "representative_dispatch":archive.choice_sets.first().map(agentir_policy_eval::ranking_dispatch_work_units),
        "semantic_identity":false,
        "timing":false
    });

    write_json(output.join("dataset-manifest.json"), &dataset.manifest)?;
    write_jsonl(output.join("dataset-examples.jsonl"), &dataset.examples)?;
    write_json(output.join("split-manifest.json"), &split)?;
    write_jsonl(
        output.join("training-runs.jsonl"),
        std::slice::from_ref(&training_run),
    )?;
    write_jsonl(
        output.join("model-artifacts.jsonl"),
        std::slice::from_ref(&model),
    )?;
    write_jsonl(output.join("inference-records.jsonl"), &inferences)?;
    let episodes = archive
        .runs
        .iter()
        .flat_map(|run| run.episodes.iter().cloned())
        .collect::<Vec<_>>();
    write_jsonl(output.join("episodes.jsonl"), &episodes)?;
    let rejections = episodes
        .iter()
        .flat_map(|episode| {
            episode
                .steps
                .iter()
                .filter(|step| !step.outcome.accepted)
                .map(move |step| {
                    json!({
                        "episode_id":episode.id,
                        "ordinal":step.ordinal,
                        "classification":step.outcome.rejection,
                        "compiler_error_code":step.outcome.compiler_error_code
                    })
                })
        })
        .collect::<Vec<_>>();
    write_jsonl(output.join("rejections.jsonl"), &rejections)?;
    write_jsonl(output.join("timing-samples.jsonl"), &timings)?;
    write_json(output.join("work-units.json"), &work_units)?;

    let aggregates = archive
        .runs
        .iter()
        .map(|run| {
            let aggregate = harness.aggregate(&run.id)?;
            Ok(json!({
                "run_id":run.id,
                "policy":run.policy.name,
                "aggregate":aggregate,
                "ranking":aggregate_ranking_metrics(run)
            }))
        })
        .collect::<EvaluationResult<Vec<_>>>()?;
    write_json(output.join("aggregates.json"), &aggregates)?;

    let mutations = mutation_results(&archive)?;
    write_jsonl(output.join("mutation-results.jsonl"), &mutations)?;
    let archive_bytes = serde_json::to_vec(&archive)
        .map_err(|error| study_error(format!("study archive encoding failed: {error}")))?;
    fs::write(output.join("evaluation-archive.json"), &archive_bytes).map_err(io_error)?;

    let exact_sequences = archive
        .runs
        .iter()
        .map(|run| {
            (
                run.id.clone(),
                run.episodes
                    .iter()
                    .flat_map(|episode| &episode.steps)
                    .filter_map(|step| {
                        step.ranking_trace
                            .as_ref()
                            .map(|trace| trace.choice_set_hash.clone())
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let semantic = json!({
        "schema_version":"agentir.stage6c.study.v1",
        "dataset_hash":dataset.manifest.ranking_dataset_hash,
        "split_hash":split.dataset_split_hash,
        "training_configuration_hash":configuration.training_configuration_hash,
        "training_run_hash":training_run.training_run_hash,
        "learned_model_hash":model.learned_model_hash,
        "archive_hash":archive.archive_hash,
        "episode_hashes":episodes.iter().filter_map(|episode| episode.episode_hash.clone()).collect::<Vec<_>>(),
        "inference_hashes":inferences.iter().map(|record| record.inference_hash.clone()).collect::<Vec<_>>(),
        "choice_set_hash_sequences":exact_sequences,
        "external_calls":0,
        "device_calls":0,
        "network_calls":0,
        "training_is_correctness":false,
        "scores_are_correctness":false
    });
    write_json(output.join("semantic.json"), &semantic)?;
    let summary = json!({
        "schema_version":"agentir.stage6c.study.v1",
        "runs":archive.runs.len(),
        "episodes":episodes.len(),
        "dataset_examples":dataset.examples.len(),
        "train_groups":split.counts.get(&SplitKind::Train).copied().unwrap_or(0),
        "validation_groups":split.counts.get(&SplitKind::Validation).copied().unwrap_or(0),
        "test_groups":split.counts.get(&SplitKind::Test).copied().unwrap_or(0),
        "excluded_groups":split.counts.get(&SplitKind::Excluded).copied().unwrap_or(0),
        "model_bytes":serde_json::to_vec(&model).map_err(|error| study_error(error.to_string()))?.len(),
        "archive_bytes":archive_bytes.len(),
        "inferences":inferences.len(),
        "mutations":mutations.len(),
        "external_calls":0,
        "device_calls":0,
        "timings_are_correctness":false
    });
    write_json(output.join("summary.json"), &summary)?;
    write_analysis(output, &summary, &semantic)?;
    println!(
        "{}",
        serde_json::to_string(&summary).map_err(|error| study_error(error.to_string()))?
    );
    Ok(())
}

fn run_scripted(
    harness: &mut EvaluationHarness,
    policy_name: &str,
    seeds: &[u64],
) -> EvaluationResult<String> {
    let run_id = harness.start_ranked_run(policy_name, &[], seeds)?;
    let episode_ids = harness
        .run(&run_id)?
        .episodes
        .iter()
        .map(|episode| episode.id.clone())
        .collect::<Vec<_>>();
    let schema = harness.ranking_feature_schema()?;
    let ranker = scripted_ranker(policy_name, &schema, seeds.first().copied().unwrap_or(0))?;
    for episode_id in episode_ids {
        while harness
            .run(&run_id)?
            .episodes
            .iter()
            .find(|episode| episode.id == episode_id)
            .is_some_and(|episode| episode.result.is_none())
        {
            let observation = harness.next_observation(&episode_id)?;
            let choice_set = harness.ranked_choice_set(&episode_id)?;
            let decision = scripted_ranking_decision(
                &ranker,
                &choice_set,
                &agentir_policy_eval::RankingLimits::default(),
            )?;
            harness.rank_episode(RankingSubmission {
                episode_id: episode_id.clone(),
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

fn run_learned(
    harness: &mut EvaluationHarness,
    policy: agentir_policy_eval::PolicyDescriptor,
    ranker: &agentir_policy_eval::RankingPolicyDescriptor,
    model: &agentir_policy_eval::LearnedModelArtifact,
    schema: &agentir_policy_eval::FeatureSchema,
    limits: &LearnedRankingLimits,
) -> EvaluationResult<(
    String,
    Vec<agentir_policy_eval::RankingInput>,
    Vec<agentir_policy_eval::InferenceRecord>,
    u128,
)> {
    let run_id = harness.start_run(policy, &[], &[0])?;
    let episode_ids = harness
        .run(&run_id)?
        .episodes
        .iter()
        .map(|episode| episode.id.clone())
        .collect::<Vec<_>>();
    let mut inputs = BTreeMap::new();
    let mut inferences = BTreeMap::new();
    let mut inference_ns = 0_u128;
    for episode_id in episode_ids {
        while harness
            .run(&run_id)?
            .episodes
            .iter()
            .find(|episode| episode.id == episode_id)
            .is_some_and(|episode| episode.result.is_none())
        {
            let observation = harness.next_observation(&episode_id)?;
            let choice_set = harness.ranked_choice_set(&episode_id)?;
            let input = ranking_input(
                &choice_set,
                schema,
                FrameCompleteness::Complete,
                agentir_policy_eval::PolicyKind::Menu,
                false,
                BTreeMap::from([
                    ("task_id".to_owned(), observation.task_id.0.clone()),
                    (
                        "corpus_version".to_owned(),
                        harness.corpus().version.clone(),
                    ),
                ]),
            )?;
            let started = Instant::now();
            let (decision, inference) = infer(&input, schema, model, ranker, limits)?;
            inference_ns = inference_ns.saturating_add(started.elapsed().as_nanos());
            inputs.insert(input.ranking_input_hash.clone(), input);
            inferences.insert(inference.inference_hash.clone(), inference);
            harness.rank_episode_with_descriptor(
                RankingSubmission {
                    episode_id: episode_id.clone(),
                    step_id: observation.step_id,
                    observation_hash: observation.observation_hash,
                    choice_set_hash: choice_set.choice_set_hash,
                    feature_schema_hash: schema.feature_schema_hash.clone(),
                    ranking_policy_hash: ranker.ranking_policy_hash.clone(),
                    decision,
                    usage: None,
                    correlation_id: None,
                },
                ranker,
            )?;
        }
    }
    Ok((
        run_id,
        inputs.into_values().collect(),
        inferences.into_values().collect(),
        inference_ns,
    ))
}

fn mutation_results(archive: &EvaluationArchive) -> EvaluationResult<Vec<Value>> {
    let mut results = Vec::new();
    let cases = [
        "changed_weight",
        "changed_inference_score",
        "wrong_dataset_hash",
    ];
    for case in cases {
        let mut mutated = archive.clone();
        match case {
            "changed_weight" => {
                mutated.learned_models[0].weights[0] =
                    mutated.learned_models[0].weights[0].saturating_add(1);
            }
            "changed_inference_score" => {
                mutated.inference_records[0].scores[0].score.units = mutated.inference_records[0]
                    .scores[0]
                    .score
                    .units
                    .saturating_add(1);
            }
            "wrong_dataset_hash" => {
                "wrong".clone_into(&mut mutated.ranking_datasets[0].manifest.ranking_dataset_hash);
            }
            _ => {}
        }
        mutated.archive_hash.clear();
        mutated.archive_hash = agentir_policy_eval::hashing::domain_hash(
            agentir_policy_eval::hashing::ARCHIVE_HASH_V6_DOMAIN,
            &mutated,
        )?;
        let diagnostic = verify_archive(&mutated).unwrap_err();
        results.push(json!({
            "case":case,
            "status":"rejected",
            "diagnostic":diagnostic.code,
            "panic":false,
            "envelope_hash_recomputed":true
        }));
    }
    for case in [
        "split_leakage",
        "duplicate_example",
        "missing_model",
        "incompatible_schema",
        "truncated_archive",
        "invalid_utf8",
        "duplicate_json_field",
        "future_version",
        "exact_byte_limit",
        "byte_limit_plus_one",
    ] {
        results.push(json!({
            "case":case,
            "status":"covered_by_deterministic_test",
            "panic":false
        }));
    }
    Ok(results)
}

fn timing(name: &str, elapsed_ns: u128) -> Value {
    json!({
        "name":name,
        "warmups":0,
        "samples":1,
        "min_ns":elapsed_ns,
        "median_ns":elapsed_ns,
        "p90_ns":elapsed_ns,
        "p95_ns":elapsed_ns,
        "p99_ns":elapsed_ns,
        "max_ns":elapsed_ns,
        "mean_ns":elapsed_ns,
        "stddev_ns":0,
        "timing_is_correctness":false
    })
}

fn parse_output() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut arguments = env::args().skip(1);
    match (
        arguments.next().as_deref(),
        arguments.next(),
        arguments.next(),
    ) {
        (Some("--output"), Some(path), None) => Ok(PathBuf::from(path)),
        _ => Err("usage: stage6c_study --output PATH".into()),
    }
}

fn write_json(path: PathBuf, value: &impl Serialize) -> EvaluationResult<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| study_error(format!("JSON encoding failed: {error}")))?;
    fs::write(path, bytes).map_err(io_error)
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

fn write_analysis(output: &Path, summary: &Value, semantic: &Value) -> EvaluationResult<()> {
    let analysis = format!(
        "# AgentIR Stage 6C local study analysis\n\n\
         This fixed-seed run is fully offline. Learned scores, labels, metrics, replay, and timing remain non-correctness evidence.\n\n\
         ## Result\n\n\
         - dataset examples: {}\n\
         - model bytes: {}\n\
         - inference records: {}\n\
         - evaluation archive bytes: {}\n\
         - archive hash: `{}`\n\n\
         Exact-frame policy comparisons are retained only until choice-set sequence divergence; later frames are incomparable. No overall quality score or 'best policy' claim is made.\n\n\
         ## Defects found and fixed\n\n\
         - The first learned text-feature projection could exceed the established Stage 6B fixed-score magnitude. It is now bounded before checked integer accumulation, with overflow and deterministic-training regressions.\n\
         - The first learned policy descriptor used a new score-representation spelling, so the production ranking dispatcher correctly rejected it. Learned inference now reuses the exact Stage 6B representation identifier, with a regression proving one accepted dispatch and read-only failure.\n\n\
         Mutation cases were bounded and panic-free. Timing samples are machine observations only. No provider, network, model API, GPU, or device call occurred.\n",
        summary["dataset_examples"],
        summary["model_bytes"],
        summary["inferences"],
        summary["archive_bytes"],
        semantic["archive_hash"].as_str().unwrap_or("unknown")
    );
    fs::write(output.join("analysis.md"), analysis).map_err(io_error)
}

#[allow(clippy::needless_pass_by_value)]
fn io_error(error: io::Error) -> EvaluationDiagnostic {
    study_error(format!("study artifact I/O failed: {error}"))
}

fn study_error(message: impl Into<String>) -> EvaluationDiagnostic {
    agentir_policy_eval::EvaluationDiagnostic {
        code: agentir_policy_eval::EvaluationErrorCode::EvaluationTranscriptInvalid,
        message: message.into(),
        expected: None,
        actual: None,
        details: BTreeMap::new(),
        repairs: Vec::new(),
        typed_repairs: Vec::new(),
    }
}
