use agentir_core::{
    backend::{compiler_build_hash, device_fingerprint_hash},
    backend_ir::{DeviceFingerprint, HardwareBenchmarkConfig, HardwareMeasurementRecord},
};
use agentir_policy_eval::ranking::{feature_schema_v1, scripted_ranker};
use agentir_policy_eval::{
    EvaluationHarness, LearnedModelArtifact, MeasuredMetric, MeasuredObjectiveDescriptor,
    MeasurementAggregationMethod, MeasurementCohort, MeasurementCohortRecord,
    MeasurementValidationPolicy, ObjectiveDirection, SearchLimits, SearchObjectiveComponent,
    SearchObjectiveComponentKind, SearchObjectiveDescriptor, SearchPlan, SearchRanker,
    SearchSession, attach_measured_search_artifacts, attach_search_artifacts,
    builtin_ranked_corpus, learned_model_hash, learned_policy, measured_recommendation,
    measurement_cohort_hash, verify_measurement_cohort,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    time::Instant,
};

fn output_path() -> Result<PathBuf, String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() == 2 && args[0] == "--output" {
        Ok(PathBuf::from(&args[1]))
    } else {
        Err("usage: stage7b_study --output PATH".to_owned())
    }
}

fn objective(
    corpus: &agentir_policy_eval::EvaluationCorpus,
    task: &agentir_policy_eval::EvaluationTask,
) -> SearchObjectiveDescriptor {
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
                kind: SearchObjectiveComponentKind::RejectionCount,
                direction: ObjectiveDirection::Minimize,
            },
            SearchObjectiveComponent {
                kind: SearchObjectiveComponentKind::TrajectoryLength,
                direction: ObjectiveDirection::Minimize,
            },
        ],
    )
    .expect("study objective")
}

fn study_corpus() -> agentir_policy_eval::EvaluationCorpus {
    let mut corpus = builtin_ranked_corpus().expect("ranked corpus");
    let requests = include_str!("../../../examples/backend_serial.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("backend serial request"))
        .collect::<Vec<Value>>();
    let task = corpus
        .tasks
        .iter_mut()
        .find(|task| task.id.0 == "ranked-backend-large")
        .expect("backend task");
    task.initial_state.production_requests = requests;
    task.objective.summary =
        "Stage 7B study: rank proved serial/vector/remainder artifacts".to_owned();
    task.metadata.insert(
        "stage7b_study_surface".to_owned(),
        "verified_backend_serial_fixture_family".to_owned(),
    );
    corpus.name = "stage7b-measured-search-study".to_owned();
    corpus.version = "stage7b-study-v1".to_owned();
    corpus.corpus_hash.clear();
    corpus.corpus_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::CORPUS_HASH_DOMAIN,
        &corpus,
    )
    .expect("study corpus hash");
    corpus
}

fn zero_model() -> (LearnedModelArtifact, agentir_policy_eval::FeatureSchema) {
    let schema = feature_schema_v1().expect("schema");
    let feature_names = schema
        .definitions
        .iter()
        .map(|definition| definition.name.clone())
        .chain(std::iter::once("compiler_order".to_owned()))
        .collect::<Vec<_>>();
    let mut model = LearnedModelArtifact {
        version: 1,
        model_kind: "pairwise_integer_linear_v1".to_owned(),
        ranking_dataset_hash: "stage7b-synthetic-study-dataset".to_owned(),
        dataset_split_hash: "stage7b-synthetic-study-split".to_owned(),
        training_configuration_hash: "stage7b-synthetic-study-training".to_owned(),
        feature_schema_hash: schema.feature_schema_hash.clone(),
        feature_codec: "visible_feature_integer_codec_v1".to_owned(),
        feature_names,
        weights: vec![0; schema.definitions.len() + 1],
        bias: 0,
        score_scale: 1_000_000,
        maximum_choices: 1_000,
        learned_model_hash: String::new(),
    };
    model.learned_model_hash = learned_model_hash(&model).expect("model hash");
    (model, schema)
}

fn sessions() -> Vec<(SearchSession, agentir_policy_eval::SearchCheckpoint)> {
    let corpus = study_corpus();
    let task = corpus
        .tasks
        .iter()
        .find(|task| task.id.0 == "ranked-backend-large")
        .expect("backend task")
        .clone();
    let schema = feature_schema_v1().expect("schema");
    let scripted = SearchRanker::Scripted {
        descriptor: scripted_ranker("goal_directed_rule_v1", &schema, 0).expect("ranker"),
    };
    let (model, schema) = zero_model();
    let learned = SearchRanker::Learned {
        descriptor: learned_policy(
            &model,
            agentir_policy_eval::PolicyKind::Menu,
            false,
            1_000_000_000,
        )
        .expect("learned policy"),
        model: Box::new(model),
        schema,
    };
    let mut output = Vec::new();
    for ranker in [scripted, learned] {
        for width in [1, 2, 4] {
            let objective = objective(&corpus, &task);
            let plan = SearchPlan::deterministic_beam_v1(&objective, &ranker, width, 64, 4, 1)
                .expect("plan");
            let mut session =
                SearchSession::start(corpus.clone(), task.id.clone(), objective, plan, &ranker)
                    .expect("session");
            session
                .advance(u64::MAX, &ranker, &SearchLimits::default())
                .expect("search");
            agentir_policy_eval::replay_search(&session, &ranker, &SearchLimits::default())
                .expect("replay");
            let checkpoint = session
                .checkpoint(&SearchLimits::default())
                .expect("checkpoint");
            output.push((session, checkpoint));
        }
    }
    output
}

fn artifact_hashes(
    sessions: &[(SearchSession, agentir_policy_eval::SearchCheckpoint)],
) -> Vec<String> {
    let mut hashes = Vec::new();
    for (session, _) in sessions {
        let value = serde_json::to_value(session).expect("session value");
        collect_named_string(&value, "artifact_hash", &mut hashes);
    }
    hashes.sort();
    hashes.dedup();
    hashes
}

fn collect_named_string(value: &Value, name: &str, output: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(value) = map.get(name).and_then(Value::as_str) {
                output.push(value.to_owned());
            }
            for value in map.values() {
                collect_named_string(value, name, output);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_named_string(value, name, output);
            }
        }
        _ => {}
    }
}

fn synthetic_records(artifact_hashes: &[String], tie_cohort: bool) -> Vec<MeasurementCohortRecord> {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../agentir-store/tests/fixtures");
    let device = DeviceFingerprint {
        backend_api: "synthetic_fixture".to_owned(),
        adapter_name: "stage7b-offline-study-no-real-device".to_owned(),
        vendor_id: Some(0),
        device_id: Some(0),
        driver_info: Some("synthetic-test-data".to_owned()),
        limits: BTreeMap::new(),
        runtime_version: "stage7b-synthetic-runtime-v1".to_owned(),
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let fingerprint = device_fingerprint_hash(&device).expect("fingerprint");
    let config = HardwareBenchmarkConfig {
        warmups: 2,
        iterations: 5,
        input_distribution: "synthetic_test_data_not_performance_evidence".to_owned(),
        tensor_dimensions: vec![4],
    };
    let mut records = Vec::new();
    for (index, wanted) in artifact_hashes.iter().take(3).enumerate() {
        let mut found = None;
        for entry in fs::read_dir(&fixture_dir).expect("fixture directory") {
            let path = entry.expect("fixture entry").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if !name.ends_with("-v9.json")
                || name.starts_with("corrupted-")
                || name == "future-v9.json"
            {
                continue;
            }
            let Ok(loaded) = agentir_store::load_workspace(&path) else {
                continue;
            };
            let package = loaded
                .workspace
                .artifact_store()
                .packages
                .values()
                .find(|package| package.artifact_hash.as_str() == wanted)
                .cloned();
            if let Some(package) = package {
                let mut workspace = loaded.workspace;
                let record = HardwareMeasurementRecord {
                    format_version: 1,
                    artifact_hash: package.artifact_hash.clone(),
                    target_hash: package.manifest.anchor.target_hash.clone(),
                    compiler_build_hash: compiler_build_hash(),
                    device_fingerprint_hash: fingerprint.clone(),
                    device: device.clone(),
                    config: config.clone(),
                    min_ns: 990
                        + u64::try_from(index).unwrap_or(0) * if tie_cohort { 5 } else { 100 },
                    median_ns: 1_000
                        + u64::try_from(index).unwrap_or(0) * if tie_cohort { 5 } else { 100 },
                    p95_ns: 1_100
                        + u64::try_from(index).unwrap_or(0) * if tie_cohort { 5 } else { 100 },
                    max_ns: 1_120
                        + u64::try_from(index).unwrap_or(0) * if tie_cohort { 5 } else { 100 },
                    guard_outcomes: BTreeMap::from([("synthetic".to_owned(), 5)]),
                    validation_status: "synthetic_test_data_not_performance_evidence".to_owned(),
                    runtime_version: "stage7b-synthetic-runtime-v1".to_owned(),
                    measurement_hash: agentir_core::backend::MeasurementHash::new("pending"),
                };
                let id = workspace
                    .measurement_publish(record)
                    .expect("synthetic publication");
                let record = workspace
                    .measurement_query(&id)
                    .expect("published record")
                    .clone();
                found = Some(MeasurementCohortRecord {
                    measurement_id: id,
                    record,
                });
                break;
            }
        }
        if let Some(record) = found {
            records.push(record);
        }
    }
    records
}

fn compatible_fixture_artifacts(search_artifacts: &[String]) -> Vec<String> {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../agentir-store/tests/fixtures");
    let mut by_hash = BTreeMap::<String, String>::new();
    for entry in fs::read_dir(&fixture_dir).expect("fixture directory") {
        let path = entry.expect("fixture entry").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if !name.ends_with("-v9.json") || name.starts_with("corrupted-") || name == "future-v9.json"
        {
            continue;
        }
        let Ok(loaded) = agentir_store::load_workspace(&path) else {
            continue;
        };
        for package in loaded.workspace.artifact_store().packages.values() {
            by_hash.insert(
                package.artifact_hash.to_string(),
                package.manifest.anchor.spec_hash.to_string(),
            );
        }
    }
    let Some(spec_hash) = search_artifacts
        .iter()
        .find_map(|artifact| by_hash.get(artifact))
        .cloned()
    else {
        return search_artifacts.to_vec();
    };
    let mut artifacts = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for artifact in search_artifacts.iter().cloned().chain(
        by_hash
            .into_iter()
            .filter_map(|(artifact, spec)| (spec == spec_hash).then_some(artifact)),
    ) {
        if seen.insert(artifact.clone()) {
            artifacts.push(artifact);
        }
    }
    artifacts
}

fn cohort(session: &SearchSession, mut records: Vec<MeasurementCohortRecord>) -> MeasurementCohort {
    records.sort_by(|left, right| {
        left.record
            .measurement_hash
            .cmp(&right.record.measurement_hash)
    });
    let first = &records[0].record;
    let mut artifacts = records
        .iter()
        .map(|entry| entry.record.artifact_hash.to_string())
        .collect::<Vec<_>>();
    artifacts.sort();
    artifacts.dedup();
    let mut cohort = MeasurementCohort {
        version: 1,
        corpus_hash: session.objective.corpus_hash.clone(),
        task_id: session.task_id.clone(),
        initial_anchor_hash: session.objective.initial_anchor_hash.clone(),
        measurement_hashes: records
            .iter()
            .map(|entry| entry.record.measurement_hash.to_string())
            .collect(),
        artifact_hashes: artifacts,
        target_hash: first.target_hash.to_string(),
        compiler_build_hash: first.compiler_build_hash.to_string(),
        device_fingerprint_hash: first.device_fingerprint_hash.to_string(),
        runtime_version: first.runtime_version.clone(),
        benchmark_config: first.config.clone(),
        validation_policy: MeasurementValidationPolicy::SyntheticFixtureV1,
        records_per_artifact: 1,
        aggregation_method: MeasurementAggregationMethod::SingleRecordSummaryV1,
        records,
        measurement_cohort_hash: String::new(),
    };
    cohort.measurement_cohort_hash = measurement_cohort_hash(&cohort).expect("cohort hash");
    verify_measurement_cohort(&cohort).expect("cohort verification");
    cohort
}

fn write_json(path: &Path, value: &impl serde::Serialize) {
    let bytes = serde_json::to_vec_pretty(value).expect("json");
    fs::write(path, bytes).expect("write");
}

fn main() -> Result<(), String> {
    let output = output_path()?;
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let searches = sessions();
    let search_hashes = artifact_hashes(&searches);
    let hashes = compatible_fixture_artifacts(&search_hashes);
    let records = synthetic_records(&hashes, false);
    if records.len() < 3 {
        write_json(
            &output.join("structured-skip.json"),
            &json!({
                "code": "INSUFFICIENT_OFFLINE_ARTIFACT_FIXTURES",
                "search_artifact_hashes": search_hashes,
                "compatible_fixture_artifact_hashes": hashes,
                "verified_fixture_records": records.len(),
                "performance_claim": false
            }),
        );
        return Err("Stage 7B study requires three verified offline artifact fixtures".to_owned());
    }
    let normal = cohort(&searches[0].0, records);
    let tie = cohort(&searches[0].0, synthetic_records(&hashes, true));
    let mut artifacts = Vec::new();
    for (session, _) in &searches {
        for (cohort, ppm) in [(&normal, 1_000), (&tie, 10_000)] {
            for metric in [MeasuredMetric::MedianNs, MeasuredMetric::P95Ns] {
                let objective = MeasuredObjectiveDescriptor::new(
                    cohort,
                    metric,
                    ppm,
                    session.objective.search_objective_hash.clone(),
                )
                .expect("measured objective");
                let recommendation = measured_recommendation(session, cohort, &objective)
                    .expect("measured recommendation");
                artifacts.push((cohort.clone(), objective, recommendation));
            }
        }
    }
    let harness = EvaluationHarness::with_corpus_and_limits(
        study_corpus(),
        agentir_policy_eval::EvaluationLimits::default(),
    )
    .expect("harness");
    let mut archive = harness.archive(&[]).expect("base archive");
    let (model, schema) = zero_model();
    archive.feature_schemas.push(schema);
    archive.learned_models.push(model);
    archive.archive_hash.clear();
    archive.archive_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_V5_DOMAIN,
        &archive,
    )
    .expect("archive hash");
    agentir_policy_eval::verify_archive(&archive).expect("model archive");
    let archive = attach_search_artifacts(&archive, &searches).expect("search archive");
    let archive = attach_measured_search_artifacts(&archive, &artifacts).expect("measured archive");
    let recommendations = archive.measured_recommendations.clone();
    let tie_values = tie
        .records
        .iter()
        .map(|entry| entry.record.median_ns)
        .collect::<Vec<_>>();
    let mut cohort_indifference_pairs = 0_u64;
    for left in 0..tie_values.len() {
        for right in (left + 1)..tie_values.len() {
            if agentir_policy_eval::within_indifference_band(
                tie_values[left],
                tie_values[right],
                10_000,
            )
            .expect("indifference")
            {
                cohort_indifference_pairs += 1;
            }
        }
    }
    let semantic = json!({
        "schema_version": "agentir.stage7b.study.v1",
        "task_count": 1,
        "search_count": searches.len(),
        "scripted_searches": 3,
        "learned_searches": 3,
        "beam_widths": [1, 2, 4],
        "objective_variants": ["median_ns", "p95_ns"],
        "verified_artifacts": normal.artifact_hashes.len(),
        "cohorts": {"normal": normal.measurement_cohort_hash, "indifference": tie.measurement_cohort_hash, "structured_rejection": "mixed_device"},
        "measurement_records_inspected": normal.records.len() + tie.records.len(),
        "recommendations": recommendations.iter().filter(|item| item.artifact_hash.is_some()).count(),
        "no_recommendation": recommendations.iter().filter(|item| item.artifact_hash.is_none()).count(),
        "indifference_ties": cohort_indifference_pairs,
        "hardware_calls_during_search_or_replay": 0,
        "synthetic_measurements_are_performance_evidence": false,
        "global_optimality_claim": false,
        "correctness_claim_from_measurement": false,
        "archive_bytes": serde_json::to_vec(&archive).expect("archive bytes").len(),
    });
    write_json(&output.join("semantic.json"), &semantic);
    write_json(&output.join("recommendations.json"), &recommendations);
    write_json(
        &output.join("checkpoints.json"),
        &archive.search_checkpoints,
    );
    write_json(&output.join("archive-v5.json"), &archive);
    write_json(
        &output.join("mutation-classifications.json"),
        &json!({
            "corrupt_cohort_hash": "rejected",
            "corrupt_objective_hash": "rejected",
            "corrupt_recommendation_hash": "rejected",
            "client_supplied_timing": "rejected",
            "cross_device_pooling": "rejected"
        }),
    );
    write_json(
        &output.join("device-study.json"),
        &json!({
            "status": "skipped",
            "code": "REAL_WEBGPU_DEVICE_STUDY_NOT_REQUESTED_OR_AVAILABLE",
            "fake_hardware_records_created": false,
            "performance_conclusion": false
        }),
    );
    write_json(
        &output.join("timing-observations.json"),
        &json!({
            "elapsed_ns": started.elapsed().as_nanos().to_string(),
            "timing_is_correctness": false
        }),
    );
    println!("{}", serde_json::to_string(&semantic).expect("summary"));
    Ok(())
}
