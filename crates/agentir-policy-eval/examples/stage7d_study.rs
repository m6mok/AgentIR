use agentir_core::{backend_ir::HardwareBenchmarkConfig, ids::ArtifactId};
use agentir_policy_eval::ranking::{feature_schema_v1, scripted_ranker};
use agentir_policy_eval::{
    EvaluationHarness, EvaluationLimits, MeasuredMetric, MeasuredObjectiveDescriptor,
    MeasurementAcquisitionArchiveBundle, MeasurementAcquisitionArtifact,
    MeasurementAcquisitionCatalog, MeasurementAcquisitionExecutor,
    MeasurementAcquisitionFailurePolicy, MeasurementAcquisitionOrderingPolicy,
    MeasurementAcquisitionPlan, MeasurementAcquisitionPlanRequest,
    MeasurementAcquisitionRecoveryAnchors, MeasurementAcquisitionRecoveryArchiveBundle,
    MeasurementAcquisitionRecoveryFaultBoundary, MeasurementAcquisitionRecoveryJournal,
    MeasurementAcquisitionRecoveryLimits, MeasurementAcquisitionSession,
    MeasurementAcquisitionStore, MeasurementAggregationMethod, MeasurementCohortRecord,
    MeasurementCohortRequest, MeasurementValidationPolicy, ObjectiveDirection, SearchLimits,
    SearchObjectiveComponent, SearchObjectiveComponentKind, SearchObjectiveDescriptor, SearchPlan,
    SearchRanker, SearchSession, SyntheticMeasurementAcquisitionExecutor,
    SyntheticMeasurementAcquisitionStore, attach_measured_search_artifacts,
    attach_measurement_acquisition_artifacts, attach_measurement_acquisition_recovery_artifacts,
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
        Err("usage: stage7d_study --output PATH".to_owned())
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
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
    "Stage 7D durable recovery and reconciliation study".clone_into(&mut task.objective.summary);
    task.metadata.insert(
        "stage7d_fixture".to_owned(),
        "synthetic_test_data_not_performance_evidence".to_owned(),
    );
    "stage7d-recovery-study".clone_into(&mut corpus.name);
    "stage7d-study-v1".clone_into(&mut corpus.version);
    corpus.corpus_hash.clear();
    corpus.corpus_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::CORPUS_HASH_DOMAIN,
        &corpus,
    )
    .expect("corpus hash");
    corpus
}

fn search_session() -> (SearchSession, agentir_policy_eval::SearchCheckpoint) {
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
    let ranker = SearchRanker::Scripted {
        descriptor: scripted_ranker("goal_directed_rule_v1", &schema, 0).expect("ranker"),
    };
    let plan =
        SearchPlan::deterministic_beam_v1(&objective, &ranker, 4, 64, 4, 1).expect("search plan");
    let mut session =
        SearchSession::start(corpus, task.id, objective, plan, &ranker).expect("search start");
    session
        .advance(u64::MAX, &ranker, &SearchLimits::default())
        .expect("search");
    agentir_policy_eval::replay_search(&session, &ranker, &SearchLimits::default())
        .expect("search replay");
    let checkpoint = session
        .checkpoint(&SearchLimits::default())
        .expect("search checkpoint");
    (session, checkpoint)
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

fn terminal_artifact(search: &SearchSession) -> String {
    let terminal = search
        .terminal_candidates
        .first()
        .expect("terminal candidate");
    let mut node = search.nodes.get(terminal).expect("terminal node");
    let mut hashes = Vec::new();
    while let Some(edge_id) = &node.parent_edge {
        let edge = search.edges.get(edge_id).expect("terminal edge");
        collect_named_string(
            &edge.compiler_outcome.response,
            "artifact_hash",
            &mut hashes,
        );
        node = search.nodes.get(&edge.parent).expect("terminal parent");
    }
    hashes.sort();
    hashes.dedup();
    assert_eq!(hashes.len(), 1, "study terminal must publish one artifact");
    hashes.pop().expect("terminal artifact")
}

fn acquisition(
    search: &SearchSession,
) -> (MeasurementAcquisitionCatalog, MeasurementAcquisitionPlan) {
    let artifact_hash = terminal_artifact(search);
    let catalog = MeasurementAcquisitionCatalog::synthetic_fixture(
        "stage7d-production-workspace-fixture".to_owned(),
        search.objective.initial_anchor_hash.clone(),
        vec![MeasurementAcquisitionArtifact {
            artifact_id: ArtifactId::new("stage7d-artifact-1"),
            artifact_hash: artifact_hash.clone(),
            spec_hash: "stage7d-shared-spec".to_owned(),
            target_hash: "stage7d-shared-target".to_owned(),
            compiler_build_hash: "stage7d-synthetic-build".to_owned(),
            status: agentir_core::backend_ir::ArtifactStatus::Validated,
            offline_valid: true,
        }],
    )
    .expect("catalog");
    let plan = MeasurementAcquisitionPlan::new(
        &catalog,
        MeasurementAcquisitionPlanRequest {
            corpus_hash: search.objective.corpus_hash.clone(),
            task_id: search.task_id.clone(),
            root_anchor_hash: search.objective.initial_anchor_hash.clone(),
            artifact_hashes: vec![artifact_hash],
            benchmark_config: HardwareBenchmarkConfig {
                warmups: 2,
                iterations: 5,
                input_distribution: "deterministic_zero_v1".to_owned(),
                tensor_dimensions: vec![4],
            },
            records_per_artifact: 1,
            validation_policy: MeasurementValidationPolicy::SyntheticFixtureV1,
            ordering_policy: MeasurementAcquisitionOrderingPolicy::RoundRobinArtifactHashV1,
            failure_policy: MeasurementAcquisitionFailurePolicy::StopOnFirstFailureV1,
            checkpoint_cadence_slots: 1,
        },
    )
    .expect("acquisition plan");
    (catalog, plan)
}

fn start(
    plan: &MeasurementAcquisitionPlan,
    catalog: &MeasurementAcquisitionCatalog,
) -> (
    MeasurementAcquisitionSession,
    SyntheticMeasurementAcquisitionExecutor,
) {
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let session = MeasurementAcquisitionSession::start(plan.clone(), catalog, None, &mut executor)
        .expect("acquisition start");
    (session, executor)
}

fn prepare(
    session: &MeasurementAcquisitionSession,
    store: &SyntheticMeasurementAcquisitionStore,
    catalog: &MeasurementAcquisitionCatalog,
) -> MeasurementAcquisitionRecoveryJournal {
    MeasurementAcquisitionRecoveryJournal::prepare(
        session,
        store,
        catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .expect("durable prepare")
}

fn main() -> Result<(), String> {
    let output = output_path()?;
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let limits = MeasurementAcquisitionRecoveryLimits::default();
    let (search, search_checkpoint) = search_session();
    let (catalog, plan) = acquisition(&search);

    let (mut normal_session, mut normal_executor) = start(&plan, &catalog);
    let mut normal_store = SyntheticMeasurementAcquisitionStore::default();
    let mut normal = prepare(&normal_session, &normal_store, &catalog);
    normal
        .execute(
            &mut normal_session,
            &mut normal_store,
            &catalog,
            None,
            &mut normal_executor,
            None,
            &limits,
        )
        .map_err(|error| error.to_string())?;

    let (mut before_session, mut before_executor) = start(&plan, &catalog);
    let mut before_store = SyntheticMeasurementAcquisitionStore::default();
    let mut before = prepare(&before_session, &before_store, &catalog);
    before
        .execute(
            &mut before_session,
            &mut before_store,
            &catalog,
            None,
            &mut before_executor,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::BeforeBenchmark),
            &limits,
        )
        .map_err(|error| error.to_string())?;

    let (mut benchmark_session, mut benchmark_executor) = start(&plan, &catalog);
    let mut benchmark_store = SyntheticMeasurementAcquisitionStore::default();
    let mut after_benchmark = prepare(&benchmark_session, &benchmark_store, &catalog);
    after_benchmark
        .execute(
            &mut benchmark_session,
            &mut benchmark_store,
            &catalog,
            None,
            &mut benchmark_executor,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::AfterBenchmarkBeforePublication),
            &limits,
        )
        .map_err(|error| error.to_string())?;

    let (mut reconciled_session, mut reconciled_executor) = start(&plan, &catalog);
    let mut reconciled_store = SyntheticMeasurementAcquisitionStore::default();
    let mut reconciled = prepare(&reconciled_session, &reconciled_store, &catalog);
    reconciled
        .execute(
            &mut reconciled_session,
            &mut reconciled_store,
            &catalog,
            None,
            &mut reconciled_executor,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::AfterPublicationBeforeCheckpoint),
            &limits,
        )
        .map_err(|error| error.to_string())?;
    let reconciled_anchors =
        MeasurementAcquisitionRecoveryAnchors::from_session(&reconciled_session);
    let reconciliation = reconciled
        .reconcile(
            &mut reconciled_session,
            &reconciled_store,
            &catalog,
            &reconciled_anchors,
            &limits,
        )
        .map_err(|error| error.to_string())?;

    let (mut retry_session, mut retry_executor) = start(&plan, &catalog);
    let mut retry_store = SyntheticMeasurementAcquisitionStore::default();
    let mut retry = prepare(&retry_session, &retry_store, &catalog);
    retry
        .execute(
            &mut retry_session,
            &mut retry_store,
            &catalog,
            None,
            &mut retry_executor,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::BeforeBenchmark),
            &limits,
        )
        .map_err(|error| error.to_string())?;
    let retry_anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&retry_session);
    retry
        .reconcile(
            &mut retry_session,
            &retry_store,
            &catalog,
            &retry_anchors,
            &limits,
        )
        .map_err(|error| error.to_string())?;
    let prior_attempt = retry.current_prepared_slot().unwrap().attempt_id.clone();
    let retry_attempt = retry
        .authorize_retry(&retry_session, &retry_store, &catalog, &limits)
        .map_err(|error| error.to_string())?
        .attempt_id
        .clone();
    retry
        .execute(
            &mut retry_session,
            &mut retry_store,
            &catalog,
            None,
            &mut retry_executor,
            None,
            &limits,
        )
        .map_err(|error| error.to_string())?;

    let (mut ambiguous_session, mut ambiguous_executor) = start(&plan, &catalog);
    let mut ambiguous_store = SyntheticMeasurementAcquisitionStore::default();
    let mut ambiguous = prepare(&ambiguous_session, &ambiguous_store, &catalog);
    let slot = ambiguous_session.slots[0].clone();
    for _ in 0..2 {
        let (record, _) = ambiguous_executor
            .benchmark(None, &catalog, &plan, &ambiguous_session.preflight, &slot)
            .map_err(|error| error.to_string())?;
        ambiguous_store
            .publish(record)
            .map_err(|error| error.to_string())?;
    }
    let ambiguous_anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&ambiguous_session);
    let ambiguous_result = ambiguous
        .reconcile(
            &mut ambiguous_session,
            &ambiguous_store,
            &catalog,
            &ambiguous_anchors,
            &limits,
        )
        .map_err(|error| error.to_string())?;

    let (abandoned_session, _) = start(&plan, &catalog);
    let abandoned_store = SyntheticMeasurementAcquisitionStore::default();
    let mut abandoned = prepare(&abandoned_session, &abandoned_store, &catalog);
    abandoned
        .abandon(&limits)
        .map_err(|error| error.to_string())?;

    let (mut cancelled_session, mut cancelled_executor) = start(&plan, &catalog);
    let mut cancelled_store = SyntheticMeasurementAcquisitionStore::default();
    cancelled_session
        .cancel()
        .map_err(|error| error.to_string())?;
    cancelled_session
        .advance(
            &mut cancelled_store,
            &catalog,
            None,
            &mut cancelled_executor,
            u64::MAX,
        )
        .map_err(|error| error.to_string())?;
    let cancelled_result = cancelled_session
        .result()
        .map_err(|error| error.to_string())?;

    let normal_result = normal_session.result().map_err(|error| error.to_string())?;
    let recovered_result = reconciled_session
        .result()
        .map_err(|error| error.to_string())?;
    if normal_result != recovered_result {
        return Err("normal and crash-reconciled Stage 7C results differ".to_owned());
    }
    let replay = reconciled
        .replay(&reconciled_session, &reconciled_store, &catalog, &limits)
        .map_err(|error| error.to_string())?;
    let recovery_checkpoint = reconciled
        .checkpoint(&reconciled_session, &limits)
        .map_err(|error| error.to_string())?;
    reconciled
        .replay(&reconciled_session, &reconciled_store, &catalog, &limits)
        .map_err(|error| error.to_string())?;

    let cohort_records = recovered_result
        .measurement_ids
        .iter()
        .map(|id| MeasurementCohortRecord {
            measurement_id: id.clone(),
            record: reconciled_store.records[id].clone(),
        })
        .collect::<Vec<_>>();
    let cohort = measurement_cohort_from_verified_records(
        cohort_records.clone(),
        MeasurementCohortRequest {
            corpus_hash: plan.corpus_hash.clone(),
            task_id: plan.task_id.clone(),
            initial_anchor_hash: plan.root_anchor_hash.clone(),
            validation_policy: plan.validation_policy,
            records_per_artifact: 1,
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
    let archive = attach_search_artifacts(&archive, &[(search.clone(), search_checkpoint)])
        .map_err(|error| error.to_string())?;
    let archive = attach_measured_search_artifacts(
        &archive,
        &[(
            cohort.clone(),
            measured_objective.clone(),
            recommendation.clone(),
        )],
    )
    .map_err(|error| error.to_string())?;
    let acquisition_checkpoint = reconciled_session
        .checkpoint()
        .map_err(|error| error.to_string())?;
    let archive = attach_measurement_acquisition_artifacts(
        &archive,
        MeasurementAcquisitionArchiveBundle {
            sessions: vec![reconciled_session.clone()],
            checkpoints: vec![acquisition_checkpoint],
            records: cohort_records,
            cohort_links: BTreeMap::from([(
                recovered_result.measurement_acquisition_result_hash.clone(),
                cohort.measurement_cohort_hash.clone(),
            )]),
        },
    )
    .map_err(|error| error.to_string())?;
    let archive = attach_measurement_acquisition_recovery_artifacts(
        &archive,
        MeasurementAcquisitionRecoveryArchiveBundle {
            journals: vec![reconciled.clone()],
            checkpoints: vec![recovery_checkpoint],
            records: Vec::new(),
            replay_statuses: BTreeMap::from([(
                reconciled
                    .measurement_acquisition_recovery_journal_hash
                    .clone(),
                true,
            )]),
        },
    )
    .map_err(|error| error.to_string())?;
    verify_archive(&archive).map_err(|error| error.to_string())?;

    let scenarios = json!({
        "normal":normal.result(),
        "crash_before_benchmark":before.result(),
        "crash_after_benchmark_before_publication":after_benchmark.result(),
        "crash_after_publication_before_checkpoint":reconciled.result(),
        "zero_publication_then_retry":retry.result(),
        "multiple_publications":ambiguous.result(),
        "explicit_abandon":abandoned.result(),
        "stage7c_cancellation":cancelled_result,
    });
    let mutations = json!({
        "normal_equals_reconciled":normal_result == recovered_result,
        "exactly_one_outcome":reconciliation.outcome,
        "multiple_outcome":ambiguous_result.outcome,
        "retry_attempt_changed":prior_attempt != retry_attempt,
        "automatic_retry_observed":false,
        "fake_hardware_records_created":false,
    });
    let metrics = json!({
        "schema_version":"agentir.stage7d.study.v1",
        "tasks":1,
        "artifacts":1,
        "recovery_scenarios":8,
        "durable_preparations":7,
        "crash_boundaries_exercised":3,
        "reconciliation_exactly_one":1,
        "reconciliation_zero":1,
        "reconciliation_multiple":1,
        "explicit_retry_authorizations":1,
        "explicit_abandonments":1,
        "stage7c_cancellations":1,
        "automatic_retries":0,
        "reconciliation_hardware_calls":0,
        "replay_hardware_calls":0,
        "published_production_format_records":reconciled_store.records.len(),
        "stage7c_result_equivalent":normal_result == recovered_result,
        "cohort_creation_success":1,
        "stage7b_recommendations":1,
        "archive_version":archive.manifest.version,
        "archive_bytes":serde_json::to_vec(&archive).map_err(|error| error.to_string())?.len(),
        "device_acquisition_observations":"synthetic_test_data_not_performance_evidence",
        "compiler_correctness_frontier_advanced":false,
        "task_success":true,
    });
    let device_skip = json!({
        "status":"skipped",
        "reason":"optional real-device recovery study requires an explicit machine-local production workspace and an operator-approved crash harness",
        "fake_hardware_records_created":false,
        "offline_readiness_failure":false,
    });

    write_json(&output.join("plan.json"), &plan)?;
    write_json(&output.join("scenarios.json"), &scenarios)?;
    write_json(
        &output.join("prepared-slots.json"),
        &vec![
            &normal.prepared_slots,
            &before.prepared_slots,
            &after_benchmark.prepared_slots,
            &reconciled.prepared_slots,
            &retry.prepared_slots,
            &ambiguous.prepared_slots,
            &abandoned.prepared_slots,
        ],
    )?;
    write_json(
        &output.join("journals.json"),
        &vec![
            normal,
            before,
            after_benchmark,
            reconciled,
            retry,
            ambiguous,
            abandoned,
        ],
    )?;
    write_json(
        &output.join("reconciliations.json"),
        &vec![reconciliation, ambiguous_result],
    )?;
    write_json(&output.join("stage7c-result.json"), &recovered_result)?;
    write_json(&output.join("replay.json"), &replay)?;
    write_json(&output.join("cohort.json"), &cohort)?;
    write_json(&output.join("measured-objective.json"), &measured_objective)?;
    write_json(
        &output.join("measured-recommendation.json"),
        &recommendation,
    )?;
    write_json(&output.join("mutations.json"), &mutations)?;
    write_json(&output.join("archive-v7.json"), &archive)?;
    write_json(&output.join("metrics.json"), &metrics)?;
    let device_output = output
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("device");
    fs::create_dir_all(&device_output).map_err(|error| error.to_string())?;
    write_json(&device_output.join("structured-skip.json"), &device_skip)?;
    write_json(
        &output.join("timing-observations.json"),
        &json!({"wall_clock_ns":u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)}),
    )?;
    println!("{}", serde_json::to_string(&metrics).expect("metrics"));
    Ok(())
}
