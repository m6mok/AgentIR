use agentir_policy_eval::hashing::{ARCHIVE_HASH_V4_DOMAIN, domain_hash};
use agentir_policy_eval::ranking::{feature_schema_v1, scripted_ranker};
use agentir_policy_eval::{
    EvaluationHarness, EvaluationTaskId, MeasuredMetric, MeasuredObjectiveDescriptor,
    MeasuredRecommendationStatus, MeasuredSearchHistoryStatus, MeasurementAggregationMethod,
    MeasurementCohortRequest, MeasurementReference, MeasurementValidationPolicy,
    ObjectiveDirection, SearchLimits, SearchObjectiveComponent, SearchObjectiveComponentKind,
    SearchObjectiveDescriptor, SearchPlan, SearchRanker, SearchSession,
    attach_measured_search_artifacts, attach_search_artifacts, builtin_corpus,
    measurement_cohort_from_workspace, migrate_archive_v4_to_v5, verify_archive,
};
use std::path::Path;

fn loaded_workspace() -> agentir_core::Workspace {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../agentir-store/tests/fixtures/measurement-record-v9.json");
    agentir_store::load_workspace(path).unwrap().workspace
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
                kind: SearchObjectiveComponentKind::TrajectoryLength,
                direction: ObjectiveDirection::Minimize,
            },
        ],
    )
    .unwrap()
}

fn finished_session() -> (SearchSession, SearchRanker) {
    let corpus = builtin_corpus().unwrap();
    let task = corpus
        .tasks
        .iter()
        .find(|task| task.id.0 == "candidate-rewrite-small")
        .unwrap()
        .clone();
    let descriptor =
        scripted_ranker("goal_directed_rule_v1", &feature_schema_v1().unwrap(), 0).unwrap();
    let ranker = SearchRanker::Scripted { descriptor };
    let objective = objective(&corpus, &task);
    let plan = SearchPlan::deterministic_beam_v1(&objective, &ranker, 2, 32, 4, 1).unwrap();
    let mut session = SearchSession::start(corpus, task.id, objective, plan, &ranker).unwrap();
    session
        .advance(u64::MAX, &ranker, &SearchLimits::default())
        .unwrap();
    (session, ranker)
}

#[test]
fn cohort_order_is_canonical_and_mixed_device_rejects() {
    let mut workspace = loaded_workspace();
    let (first_id, first_record) = workspace
        .measurement_store()
        .records
        .first_key_value()
        .map(|(id, record)| (id.clone(), record.clone()))
        .unwrap();
    let mut second_record = first_record.clone();
    second_record.min_ns = 20;
    second_record.median_ns = 21;
    second_record.p95_ns = 22;
    second_record.max_ns = 22;
    second_record.validation_status = "synthetic_test_data_not_performance_evidence".to_owned();
    let second_id = workspace.measurement_publish(second_record).unwrap();
    let cohort_left = measurement_cohort_from_workspace(
        &workspace,
        &[
            MeasurementReference::Id(first_id.clone()),
            MeasurementReference::Id(second_id.clone()),
        ],
        MeasurementCohortRequest {
            corpus_hash: "corpus".to_owned(),
            task_id: EvaluationTaskId("task".to_owned()),
            initial_anchor_hash: "root".to_owned(),
            validation_policy: MeasurementValidationPolicy::SyntheticFixtureV1,
            records_per_artifact: 2,
            aggregation_method: MeasurementAggregationMethod::MedianOfRecordSummariesV1,
        },
    )
    .unwrap();
    let cohort_right = measurement_cohort_from_workspace(
        &workspace,
        &[
            MeasurementReference::Id(second_id),
            MeasurementReference::Id(first_id),
        ],
        MeasurementCohortRequest {
            corpus_hash: "corpus".to_owned(),
            task_id: EvaluationTaskId("task".to_owned()),
            initial_anchor_hash: "root".to_owned(),
            validation_policy: MeasurementValidationPolicy::SyntheticFixtureV1,
            records_per_artifact: 2,
            aggregation_method: MeasurementAggregationMethod::MedianOfRecordSummariesV1,
        },
    )
    .unwrap();
    assert_eq!(cohort_left, cohort_right);

    let mut mixed = first_record;
    mixed.device.adapter_name = "different-synthetic-device".to_owned();
    mixed.device_fingerprint_hash =
        agentir_core::backend::device_fingerprint_hash(&mixed.device).unwrap();
    mixed.validation_status = "synthetic_test_data_not_performance_evidence".to_owned();
    let mixed_id = workspace.measurement_publish(mixed).unwrap();
    let error = measurement_cohort_from_workspace(
        &workspace,
        &[
            MeasurementReference::Hash(cohort_left.measurement_hashes[0].clone()),
            MeasurementReference::Id(mixed_id),
        ],
        MeasurementCohortRequest {
            corpus_hash: "corpus".to_owned(),
            task_id: EvaluationTaskId("task".to_owned()),
            initial_anchor_hash: "root".to_owned(),
            validation_policy: MeasurementValidationPolicy::SyntheticFixtureV1,
            records_per_artifact: 1,
            aggregation_method: MeasurementAggregationMethod::SingleRecordSummaryV1,
        },
    )
    .unwrap_err();
    assert_eq!(
        error.code,
        agentir_policy_eval::EvaluationErrorCode::EvaluationMeasurementMixedDevice
    );
}

#[test]
fn unmeasured_terminal_is_typed_and_archive_v5_replays_without_hardware() {
    let (session, _) = finished_session();
    let workspace = loaded_workspace();
    let measurement = workspace
        .measurement_store()
        .records
        .first_key_value()
        .unwrap()
        .0
        .clone();
    let cohort = measurement_cohort_from_workspace(
        &workspace,
        &[MeasurementReference::Id(measurement)],
        MeasurementCohortRequest {
            corpus_hash: session.objective.corpus_hash.clone(),
            task_id: session.task_id.clone(),
            initial_anchor_hash: session.objective.initial_anchor_hash.clone(),
            validation_policy: MeasurementValidationPolicy::SyntheticFixtureV1,
            records_per_artifact: 1,
            aggregation_method: MeasurementAggregationMethod::SingleRecordSummaryV1,
        },
    )
    .unwrap();
    let objective = MeasuredObjectiveDescriptor::new(
        &cohort,
        MeasuredMetric::MedianNs,
        1_000,
        session.objective.search_objective_hash.clone(),
    )
    .unwrap();
    let recommendation =
        agentir_policy_eval::measured_recommendation(&session, &cohort, &objective).unwrap();
    assert_eq!(
        recommendation.status,
        MeasuredRecommendationStatus::NoComparableMeasuredTerminal
    );
    assert!(recommendation.objective_value_ns.is_none());
    assert!(
        recommendation
            .ineligible_alternatives
            .iter()
            .all(|alternative| alternative.objective_value_ns.is_none())
    );

    let mut harness = EvaluationHarness::new().unwrap();
    let run = harness
        .run_scripted(
            "free_reference_v1",
            &[EvaluationTaskId("saxpy-end-to-end-large".to_owned())],
            &[0],
        )
        .unwrap();
    let archive = harness.archive(&[run]).unwrap();
    let checkpoint = session.checkpoint(&SearchLimits::default()).unwrap();
    let archive = attach_search_artifacts(&archive, &[(session, checkpoint)]).unwrap();
    let archive =
        attach_measured_search_artifacts(&archive, &[(cohort, objective, recommendation)]).unwrap();
    assert_eq!(
        archive.measured_search_history_status,
        MeasuredSearchHistoryStatus::MeasuredSearchHistoryPresent
    );
    assert_eq!(archive.measured_search_runs[0].work.hardware_calls, 0);
    verify_archive(&archive).unwrap();
}

#[test]
fn v4_to_v5_migration_is_pure_and_protocol_rejects_client_timing_data() {
    let harness = EvaluationHarness::new().unwrap();
    let mut legacy = harness.archive(&[]).unwrap();
    legacy.manifest.version = 4;
    legacy.measured_search_history_status = MeasuredSearchHistoryStatus::Unspecified;
    legacy.archive_hash.clear();
    legacy.archive_hash = domain_hash(ARCHIVE_HASH_V4_DOMAIN, &legacy).unwrap();
    verify_archive(&legacy).unwrap();
    let migrated = migrate_archive_v4_to_v5(&legacy).unwrap();
    assert_eq!(migrated.manifest.version, 5);
    assert_eq!(
        migrated.measured_search_history_status,
        MeasuredSearchHistoryStatus::NoMeasuredSearchHistory
    );
    assert!(migrated.measurement_cohorts.is_empty());

    let mut protocol = agentir_policy_eval::EvaluationProtocol::new().unwrap();
    let response: serde_json::Value = serde_json::from_str(&protocol.process_line(
        r#"{"command":"evaluation.measurement_cohort.create","request_id":"r1","task":"candidate-rewrite-small","initial_anchor_hash":"x","measurements":[],"validation_policy":"hardware_executed_v1","records_per_artifact":1,"aggregation_method":"single_record_summary_v1","median_ns":1,"device_fingerprint_hash":"forbidden"}"#,
    ))
    .unwrap();
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "EVALUATION_TRANSCRIPT_INVALID");
}
