use agentir_core::{
    Workspace,
    backend_ir::{ArtifactStatus, HardwareBenchmarkConfig, HardwareMeasurementRecord},
    ids::ArtifactId,
};
use agentir_policy_eval::ranking::{feature_schema_v1, scripted_ranker};
use agentir_policy_eval::{
    EvaluationDiagnostic, EvaluationErrorCode, EvaluationHarness, EvaluationLimits, MeasuredMetric,
    MeasuredObjectiveDescriptor, MeasurementAcquisitionArchiveBundle,
    MeasurementAcquisitionArtifact, MeasurementAcquisitionCatalog, MeasurementAcquisitionExecutor,
    MeasurementAcquisitionFailurePolicy, MeasurementAcquisitionOrderingPolicy,
    MeasurementAcquisitionPlan, MeasurementAcquisitionPlanRequest, MeasurementAcquisitionPreflight,
    MeasurementAcquisitionSession, MeasurementAcquisitionSlot, MeasurementAcquisitionStatus,
    MeasurementAggregationMethod, MeasurementCohortRecord, MeasurementCohortRequest,
    MeasurementValidationPolicy, ObjectiveDirection, SearchLimits, SearchObjectiveComponent,
    SearchObjectiveComponentKind, SearchObjectiveDescriptor, SearchPlan, SearchRanker,
    SearchSession, SyntheticMeasurementAcquisitionExecutor, SyntheticMeasurementAcquisitionStore,
    attach_measured_search_artifacts, attach_measurement_acquisition_artifacts,
    attach_search_artifacts, builtin_ranked_corpus, measured_recommendation,
    measurement_cohort_from_verified_records, verify_archive,
};
use serde::Serialize;
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
        Err("usage: stage7c_study --output PATH".to_owned())
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn study_corpus() -> agentir_policy_eval::EvaluationCorpus {
    let mut corpus = builtin_ranked_corpus().expect("ranked corpus");
    let requests = include_str!("../../../examples/backend_serial.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("backend request"))
        .collect::<Vec<Value>>();
    let task = corpus
        .tasks
        .iter_mut()
        .find(|task| task.id.0 == "ranked-backend-large")
        .expect("backend task");
    task.initial_state.production_requests = requests;
    "Stage 7C synthetic acquisition orchestration study".clone_into(&mut task.objective.summary);
    task.metadata.insert(
        "stage7c_fixture".to_owned(),
        "synthetic_test_data_not_performance_evidence".to_owned(),
    );
    "stage7c-acquisition-study".clone_into(&mut corpus.name);
    "stage7c-study-v1".clone_into(&mut corpus.version);
    corpus.corpus_hash.clear();
    corpus.corpus_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::CORPUS_HASH_DOMAIN,
        &corpus,
    )
    .expect("corpus hash");
    corpus
}

fn search_sessions() -> Vec<(SearchSession, agentir_policy_eval::SearchCheckpoint)> {
    let corpus = study_corpus();
    let task = corpus
        .tasks
        .iter()
        .find(|task| task.id.0 == "ranked-backend-large")
        .expect("task")
        .clone();
    let objective = SearchObjectiveDescriptor::new(
        &corpus,
        &task,
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
                kind: SearchObjectiveComponentKind::TrajectoryLength,
                direction: ObjectiveDirection::Minimize,
            },
        ],
    )
    .expect("objective");
    let schema = feature_schema_v1().expect("schema");
    [
        ("goal_directed_rule_v1", 4_u64, 0_u64),
        ("lexicographic_choice_v1", 2_u64, 0_u64),
        ("seeded_uniform_choice_v1", 4_u64, 7_u64),
    ]
    .into_iter()
    .map(|(name, width, seed)| {
        let ranker = SearchRanker::Scripted {
            descriptor: scripted_ranker(name, &schema, seed).expect("ranker"),
        };
        let plan = SearchPlan::deterministic_beam_v1(&objective, &ranker, width, 64, 4, 1)
            .expect("search plan");
        let mut session = SearchSession::start(
            corpus.clone(),
            task.id.clone(),
            objective.clone(),
            plan,
            &ranker,
        )
        .expect("search start");
        session
            .advance(u64::MAX, &ranker, &SearchLimits::default())
            .expect("search");
        agentir_policy_eval::replay_search(&session, &ranker, &SearchLimits::default())
            .expect("search replay");
        let checkpoint = session
            .checkpoint(&SearchLimits::default())
            .expect("search checkpoint");
        (session, checkpoint)
    })
    .collect()
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

fn artifact_hashes(
    sessions: &[(SearchSession, agentir_policy_eval::SearchCheckpoint)],
) -> Vec<String> {
    let mut hashes = Vec::new();
    for (session, _) in sessions {
        collect_named_string(
            &serde_json::to_value(session).expect("search JSON"),
            "artifact_hash",
            &mut hashes,
        );
    }
    hashes.sort();
    hashes.dedup();
    hashes
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
    artifacts.truncate(3);
    assert_eq!(artifacts.len(), 3, "study requires three proved artifacts");
    artifacts
}

fn acquisition_catalog(
    search: &SearchSession,
    artifact_hashes: &[String],
) -> MeasurementAcquisitionCatalog {
    let artifacts = artifact_hashes
        .iter()
        .enumerate()
        .map(|(index, hash)| MeasurementAcquisitionArtifact {
            artifact_id: ArtifactId::new(format!("stage7c-artifact-{}", index + 1)),
            artifact_hash: hash.clone(),
            spec_hash: "stage7c-shared-spec".to_owned(),
            target_hash: "stage7c-shared-target".to_owned(),
            compiler_build_hash: "stage7c-synthetic-build".to_owned(),
            status: ArtifactStatus::Validated,
            offline_valid: true,
        })
        .collect();
    MeasurementAcquisitionCatalog::synthetic_fixture(
        "stage7c-production-workspace-fixture".to_owned(),
        search.objective.initial_anchor_hash.clone(),
        artifacts,
    )
    .expect("catalog")
}

fn acquisition_plan(
    search: &SearchSession,
    catalog: &MeasurementAcquisitionCatalog,
    artifact_hashes: &[String],
) -> MeasurementAcquisitionPlan {
    MeasurementAcquisitionPlan::new(
        catalog,
        MeasurementAcquisitionPlanRequest {
            corpus_hash: search.objective.corpus_hash.clone(),
            task_id: search.task_id.clone(),
            root_anchor_hash: search.objective.initial_anchor_hash.clone(),
            artifact_hashes: artifact_hashes.iter().rev().cloned().collect(),
            benchmark_config: HardwareBenchmarkConfig {
                warmups: 2,
                iterations: 5,
                input_distribution: "deterministic_zero_v1".to_owned(),
                tensor_dimensions: vec![4],
            },
            records_per_artifact: 3,
            validation_policy: MeasurementValidationPolicy::SyntheticFixtureV1,
            ordering_policy: MeasurementAcquisitionOrderingPolicy::RoundRobinArtifactHashV1,
            failure_policy: MeasurementAcquisitionFailurePolicy::StopOnFirstFailureV1,
            checkpoint_cadence_slots: 1,
        },
    )
    .expect("acquisition plan")
}

fn complete(
    plan: &MeasurementAcquisitionPlan,
    catalog: &MeasurementAcquisitionCatalog,
    chunks: &[u64],
) -> (
    MeasurementAcquisitionSession,
    SyntheticMeasurementAcquisitionStore,
    SyntheticMeasurementAcquisitionExecutor,
) {
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let mut session =
        MeasurementAcquisitionSession::start(plan.clone(), catalog, None, &mut executor)
            .expect("acquisition start");
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    for chunk in chunks {
        session
            .advance(&mut store, catalog, None, &mut executor, *chunk)
            .expect("acquisition advance");
    }
    if session.status == MeasurementAcquisitionStatus::Running {
        session
            .advance(&mut store, catalog, None, &mut executor, u64::MAX)
            .expect("acquisition completion");
    }
    (session, store, executor)
}

fn unit_resume(
    plan: &MeasurementAcquisitionPlan,
    catalog: &MeasurementAcquisitionCatalog,
) -> (
    MeasurementAcquisitionSession,
    SyntheticMeasurementAcquisitionStore,
) {
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let mut session =
        MeasurementAcquisitionSession::start(plan.clone(), catalog, None, &mut executor)
            .expect("start");
    let preflight = session.preflight.clone();
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    while session.status == MeasurementAcquisitionStatus::Running {
        session
            .advance(&mut store, catalog, None, &mut executor, 1)
            .expect("unit advance");
        if session.status == MeasurementAcquisitionStatus::Running {
            let checkpoint = session.checkpoint().expect("unit checkpoint");
            session =
                MeasurementAcquisitionSession::resume(&checkpoint, &store, catalog, &preflight)
                    .expect("unit resume");
        }
    }
    (session, store)
}

#[derive(Default)]
struct UnavailableExecutor;

impl MeasurementAcquisitionExecutor for UnavailableExecutor {
    fn preflight(
        &mut self,
        _workspace: Option<&Workspace>,
        _catalog: &MeasurementAcquisitionCatalog,
        _plan: &MeasurementAcquisitionPlan,
    ) -> Result<MeasurementAcquisitionPreflight, EvaluationDiagnostic> {
        Err(EvaluationDiagnostic {
            code: EvaluationErrorCode::EvaluationAcquisitionDeviceUnavailable,
            message: "synthetic preflight reports device unavailable".to_owned(),
            expected: None,
            actual: None,
            details: BTreeMap::new(),
            repairs: Vec::new(),
            typed_repairs: Vec::new(),
        })
    }

    fn benchmark(
        &mut self,
        _workspace: Option<&Workspace>,
        _catalog: &MeasurementAcquisitionCatalog,
        _plan: &MeasurementAcquisitionPlan,
        _preflight: &MeasurementAcquisitionPreflight,
        _slot: &MeasurementAcquisitionSlot,
    ) -> Result<(HardwareMeasurementRecord, u64), EvaluationDiagnostic> {
        unreachable!("unavailable preflight publishes no session")
    }
}

fn main() -> Result<(), String> {
    let output = output_path()?;
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let search_sessions = search_sessions();
    let (search, _) = search_sessions[0].clone();
    let artifacts = compatible_fixture_artifacts(&artifact_hashes(&search_sessions));
    let catalog = acquisition_catalog(&search, &artifacts);
    let plan = acquisition_plan(&search, &catalog, &artifacts);

    let (single, single_store, single_executor) = complete(&plan, &catalog, &[u64::MAX]);
    let (split, _, _) = complete(&plan, &catalog, &[2, 1, 3, 1, 2]);
    let (unit, _) = unit_resume(&plan, &catalog);
    let single_result = single.result().map_err(|error| error.to_string())?;
    let split_result = split.result().map_err(|error| error.to_string())?;
    let unit_result = unit.result().map_err(|error| error.to_string())?;
    if single_result.measurement_acquisition_result_hash
        != split_result.measurement_acquisition_result_hash
        || single_result.measurement_acquisition_result_hash
            != unit_result.measurement_acquisition_result_hash
    {
        return Err("single/split/resume semantic results differ".to_owned());
    }

    let mut cancel_executor = SyntheticMeasurementAcquisitionExecutor::new();
    let mut cancelled =
        MeasurementAcquisitionSession::start(plan.clone(), &catalog, None, &mut cancel_executor)
            .map_err(|error| error.to_string())?;
    let cancel_preflight = cancelled.preflight.clone();
    let mut cancel_store = SyntheticMeasurementAcquisitionStore::default();
    cancelled
        .advance(&mut cancel_store, &catalog, None, &mut cancel_executor, 4)
        .map_err(|error| error.to_string())?;
    let before_cancel = cancelled.checkpoint().map_err(|error| error.to_string())?;
    cancelled.cancel().map_err(|error| error.to_string())?;
    let mut resumed_cancel = MeasurementAcquisitionSession::resume(
        &before_cancel,
        &cancel_store,
        &catalog,
        &cancel_preflight,
    )
    .map_err(|error| error.to_string())?;
    resumed_cancel
        .advance(
            &mut cancel_store,
            &catalog,
            None,
            &mut cancel_executor,
            u64::MAX,
        )
        .map_err(|error| error.to_string())?;

    let mut failure_executor = SyntheticMeasurementAcquisitionExecutor::new();
    failure_executor.fail_slot = Some(4);
    let mut failed =
        MeasurementAcquisitionSession::start(plan.clone(), &catalog, None, &mut failure_executor)
            .map_err(|error| error.to_string())?;
    let mut failure_store = SyntheticMeasurementAcquisitionStore::default();
    failed
        .advance(
            &mut failure_store,
            &catalog,
            None,
            &mut failure_executor,
            u64::MAX,
        )
        .map_err(|error| error.to_string())?;
    let unavailable = MeasurementAcquisitionSession::start(
        plan.clone(),
        &catalog,
        None,
        &mut UnavailableExecutor,
    )
    .expect_err("unavailable device must not publish a session");

    let replay_before = single_executor.invocations;
    let replay = single
        .replay(&single_store, &catalog)
        .map_err(|error| error.to_string())?;
    if replay != single_result || single_executor.invocations != replay_before {
        return Err("offline replay changed result or invoked executor".to_owned());
    }
    let cohort_records = single_result
        .measurement_ids
        .iter()
        .map(|id| MeasurementCohortRecord {
            measurement_id: id.clone(),
            record: single_store.records[id].clone(),
        })
        .collect::<Vec<_>>();
    let cohort = measurement_cohort_from_verified_records(
        cohort_records.clone(),
        MeasurementCohortRequest {
            corpus_hash: plan.corpus_hash.clone(),
            task_id: plan.task_id.clone(),
            initial_anchor_hash: plan.root_anchor_hash.clone(),
            validation_policy: plan.validation_policy,
            records_per_artifact: plan.records_per_artifact,
            aggregation_method: MeasurementAggregationMethod::MedianOfRecordSummariesV1,
        },
    )
    .map_err(|error| error.to_string())?;
    let measured_objective = MeasuredObjectiveDescriptor::new(
        &cohort,
        MeasuredMetric::MedianNs,
        1_000,
        search.objective.search_objective_hash.clone(),
    )
    .map_err(|error| error.to_string())?;
    let recommendation = measured_recommendation(&search, &cohort, &measured_objective)
        .map_err(|error| error.to_string())?;

    let harness =
        EvaluationHarness::with_corpus_and_limits(study_corpus(), EvaluationLimits::default())
            .map_err(|error| error.to_string())?;
    let archive = harness.archive(&[]).map_err(|error| error.to_string())?;
    let archive =
        attach_search_artifacts(&archive, &search_sessions).map_err(|error| error.to_string())?;
    let archive = attach_measured_search_artifacts(
        &archive,
        &[(
            cohort.clone(),
            measured_objective.clone(),
            recommendation.clone(),
        )],
    )
    .map_err(|error| error.to_string())?;
    let final_checkpoint = single.checkpoint().map_err(|error| error.to_string())?;
    let archive = attach_measurement_acquisition_artifacts(
        &archive,
        MeasurementAcquisitionArchiveBundle {
            sessions: vec![single.clone()],
            checkpoints: vec![final_checkpoint.clone()],
            records: cohort_records,
            cohort_links: BTreeMap::from([(
                single_result.measurement_acquisition_result_hash.clone(),
                cohort.measurement_cohort_hash.clone(),
            )]),
        },
    )
    .map_err(|error| error.to_string())?;
    verify_archive(&archive).map_err(|error| error.to_string())?;

    let mut corrupt = final_checkpoint.clone();
    corrupt.next_slot = corrupt.next_slot.saturating_add(1);
    let corrupt_code =
        MeasurementAcquisitionSession::resume(&corrupt, &single_store, &catalog, &single.preflight)
            .expect_err("corrupt checkpoint")
            .code;
    let mutations = json!({
        "corrupt_checkpoint": corrupt_code,
        "benchmark_failure": failed.slots[4].failure_code,
        "device_unavailable": unavailable.code,
        "numeric_failure_sentinel_used": false,
    });
    let metrics = json!({
        "schema_version":"agentir.stage7c.study.v1",
        "tasks":1,
        "artifacts":artifacts.len(),
        "plans":1,
        "sessions":6,
        "running":0,
        "complete":4,
        "cancelled":1,
        "unavailable":1,
        "failed":1,
        "total_slots":plan.total_slots().map_err(|error| error.to_string())?,
        "completed_slots":single.work.slots_completed,
        "failed_slots":failed.work.slots_failed,
        "indeterminate_slots":0,
        "rounds":plan.records_per_artifact,
        "records_per_artifact":plan.records_per_artifact,
        "checkpoints":10,
        "resumes":unit.work.resumes + resumed_cancel.work.resumes,
        "cancellations":1,
        "benchmark_invocations":single.work.benchmark_invocations,
        "real_device_calls":0,
        "synthetic_fixture_invocations":single.work.synthetic_fixture_invocations,
        "published_measurement_records":single.work.published_measurement_records,
        "prevented_reruns":unit.work.prevented_reruns + resumed_cancel.work.prevented_reruns,
        "replay_count":1,
        "replay_hardware_calls":0,
        "cohort_creation_success":1,
        "cohort_creation_failures":0,
        "stage7b_recommendations":1,
        "stage7b_no_recommendation":u64::from(recommendation.artifact_hash.is_none()),
        "resource_budget_rejections":0,
        "archive_bytes":serde_json::to_vec(&archive).map_err(|error| error.to_string())?.len(),
        "timing_median_p95_p99":"synthetic observations only; no performance evidence",
        "compiler_proof_status":"retained proved artifact provenance",
        "artifact_structural_validation":"offline_validated fixture contract",
        "device_acquisition_observations":"synthetic_test_data_not_performance_evidence",
        "acquisition_replay_validity":true,
        "cohort_eligibility":true,
        "measured_recommendation":recommendation.status,
        "task_success":true,
        "hardware_performance_observations":"none",
    });
    let device_skip = json!({
        "status":"skipped",
        "reason":"optional real-device study requires an explicit machine-local production workspace containing at least two compatible proved artifacts",
        "fake_hardware_records_created":false,
        "offline_readiness_failure":false,
    });

    write_json(&output.join("plan.json"), &plan)?;
    write_json(
        &output.join("slot-schedule.json"),
        &plan.slots().map_err(|error| error.to_string())?,
    )?;
    write_json(&output.join("synthetic-measurements.json"), &single_store)?;
    write_json(
        &output.join("checkpoints.json"),
        &vec![final_checkpoint, before_cancel],
    )?;
    write_json(
        &output.join("traces.json"),
        &vec![&single.trace, &split.trace, &unit.trace, &failed.trace],
    )?;
    write_json(
        &output.join("results.json"),
        &vec![
            single_result,
            split_result,
            unit_result,
            resumed_cancel.result().map_err(|error| error.to_string())?,
            failed.result().map_err(|error| error.to_string())?,
        ],
    )?;
    write_json(&output.join("replay.json"), &replay)?;
    write_json(&output.join("cohort.json"), &cohort)?;
    write_json(&output.join("measured-objective.json"), &measured_objective)?;
    write_json(
        &output.join("measured-recommendation.json"),
        &recommendation,
    )?;
    write_json(&output.join("mutations.json"), &mutations)?;
    write_json(&output.join("archive-v6.json"), &archive)?;
    write_json(&output.join("metrics.json"), &metrics)?;
    write_json(&output.join("device-skip.json"), &device_skip)?;
    write_json(
        &output.join("timing-observations.json"),
        &json!({"wall_clock_ns":u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)}),
    )?;
    println!("{}", serde_json::to_string(&metrics).expect("metrics"));
    Ok(())
}
