use agentir_core::{
    backend_ir::{ArtifactStatus, HardwareBenchmarkConfig},
    ids::ArtifactId,
};
use agentir_policy_eval::{
    EvaluationErrorCode, EvaluationHarness, EvaluationTaskId, MeasurementAcquisitionArtifact,
    MeasurementAcquisitionCatalog, MeasurementAcquisitionExecutor,
    MeasurementAcquisitionFailurePolicy, MeasurementAcquisitionOrderingPolicy,
    MeasurementAcquisitionPlan, MeasurementAcquisitionPlanRequest,
    MeasurementAcquisitionRecoveryAnchors, MeasurementAcquisitionRecoveryArchiveBundle,
    MeasurementAcquisitionRecoveryFaultBoundary, MeasurementAcquisitionRecoveryHistoryStatus,
    MeasurementAcquisitionRecoveryJournal, MeasurementAcquisitionRecoveryLimits,
    MeasurementAcquisitionSession, MeasurementAcquisitionStore, MeasurementValidationPolicy,
    ReconciliationOutcome, RecoveryStatus, SyntheticMeasurementAcquisitionExecutor,
    SyntheticMeasurementAcquisitionStore, attach_measurement_acquisition_recovery_artifacts,
    migrate_archive_v1_to_v2, migrate_archive_v2_to_v3, migrate_archive_v3_to_v4,
    migrate_archive_v4_to_v5, migrate_archive_v5_to_v6, migrate_archive_v6_to_v7,
    migrate_archive_v7_to_v8, verify_archive,
};
use std::collections::BTreeMap;

fn artifact(hash: &str) -> MeasurementAcquisitionArtifact {
    MeasurementAcquisitionArtifact {
        artifact_id: ArtifactId::new(format!("artifact-{hash}")),
        artifact_hash: hash.to_owned(),
        spec_hash: "spec-7d".to_owned(),
        target_hash: "target-7d".to_owned(),
        compiler_build_hash: "build-7d".to_owned(),
        status: ArtifactStatus::Validated,
        offline_valid: true,
    }
}

fn catalog() -> MeasurementAcquisitionCatalog {
    MeasurementAcquisitionCatalog::synthetic_fixture(
        "workspace-7d".to_owned(),
        "root-7d".to_owned(),
        vec![artifact("a"), artifact("b")],
    )
    .unwrap()
}

fn request(hashes: &[&str], records: u64) -> MeasurementAcquisitionPlanRequest {
    MeasurementAcquisitionPlanRequest {
        corpus_hash: "corpus-7d".to_owned(),
        task_id: EvaluationTaskId("task-7d".to_owned()),
        root_anchor_hash: "root-7d".to_owned(),
        artifact_hashes: hashes.iter().map(|hash| (*hash).to_owned()).collect(),
        benchmark_config: HardwareBenchmarkConfig {
            warmups: 1,
            iterations: 3,
            input_distribution: "deterministic_zero_v1".to_owned(),
            tensor_dimensions: vec![4],
        },
        records_per_artifact: records,
        validation_policy: MeasurementValidationPolicy::SyntheticFixtureV1,
        ordering_policy: MeasurementAcquisitionOrderingPolicy::RoundRobinArtifactHashV1,
        failure_policy: MeasurementAcquisitionFailurePolicy::StopOnFirstFailureV1,
        checkpoint_cadence_slots: 1,
    }
}

fn fixture(
    hashes: &[&str],
    records: u64,
) -> (
    MeasurementAcquisitionCatalog,
    MeasurementAcquisitionSession,
    SyntheticMeasurementAcquisitionStore,
    SyntheticMeasurementAcquisitionExecutor,
) {
    let catalog = catalog();
    let plan = MeasurementAcquisitionPlan::new(&catalog, request(hashes, records)).unwrap();
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let session =
        MeasurementAcquisitionSession::start(plan, &catalog, None, &mut executor).unwrap();
    (
        catalog,
        session,
        SyntheticMeasurementAcquisitionStore::default(),
        executor,
    )
}

fn post_publication_crash() -> (
    MeasurementAcquisitionCatalog,
    MeasurementAcquisitionSession,
    SyntheticMeasurementAcquisitionStore,
    SyntheticMeasurementAcquisitionExecutor,
    MeasurementAcquisitionRecoveryJournal,
) {
    let (catalog, mut session, mut store, mut executor) = fixture(&["a"], 1);
    let mut journal = MeasurementAcquisitionRecoveryJournal::prepare(
        &session,
        &store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    journal
        .execute(
            &mut session,
            &mut store,
            &catalog,
            None,
            &mut executor,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::AfterPublicationBeforeCheckpoint),
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    (catalog, session, store, executor, journal)
}

#[test]
fn hashes_are_stable_and_domain_separated() {
    let (catalog, session, store, executor) = fixture(&["a"], 1);
    let journal = MeasurementAcquisitionRecoveryJournal::prepare(
        &session,
        &store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    assert_eq!(executor.invocations, 1, "prepare performs no executor call");
    assert_eq!(journal.status, RecoveryStatus::Prepared);
    assert_eq!(journal.prepared_slots.len(), 1);
    assert_eq!(
        journal.prepared_slots[0].measurement_acquisition_prepared_slot_hash,
        "6bee9a954b5caeecc7c716b3b65f3a48ff99d8ee07677b79fa4ef2494f90f7c3"
    );
    assert_eq!(
        journal.measurement_acquisition_recovery_journal_hash,
        "cf2c6842dcc887d484a3f489921e6cf4762f4efafcf2be9f95fd968bfb611931"
    );
    let domains = [
        agentir_policy_eval::MEASUREMENT_ACQUISITION_RECOVERY_JOURNAL_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_ACQUISITION_PREPARED_SLOT_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_ACQUISITION_RECONCILIATION_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_ACQUISITION_PLAN_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_ACQUISITION_RESULT_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_COHORT_HASH_DOMAIN,
    ];
    assert_eq!(
        domains
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        domains.len()
    );
}

#[test]
fn prepare_and_pre_benchmark_crash_make_zero_hardware_calls() {
    let (catalog, mut session, mut store, mut executor) = fixture(&["a"], 1);
    let invocations = executor.invocations;
    let mut journal = MeasurementAcquisitionRecoveryJournal::prepare(
        &session,
        &store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    assert_eq!(executor.invocations, invocations);
    journal
        .execute(
            &mut session,
            &mut store,
            &catalog,
            None,
            &mut executor,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::BeforeBenchmark),
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    assert_eq!(executor.invocations, invocations);
    assert!(store.records.is_empty());
    assert_eq!(journal.status, RecoveryStatus::IndeterminateAfterCrash);
}

#[test]
fn crash_after_benchmark_publishes_no_fake_measurement_and_never_auto_retries() {
    let (catalog, mut session, mut store, mut executor) = fixture(&["a"], 1);
    let mut journal = MeasurementAcquisitionRecoveryJournal::prepare(
        &session,
        &store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    journal
        .execute(
            &mut session,
            &mut store,
            &catalog,
            None,
            &mut executor,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::AfterBenchmarkBeforePublication),
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    assert!(store.records.is_empty());
    assert!(session.slots[0].measurement_id.is_none());
    let before = executor.invocations;
    let anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&session);
    let result = journal
        .reconcile(
            &mut session,
            &store,
            &catalog,
            &anchors,
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    assert_eq!(result.outcome, ReconciliationOutcome::NoPublicationObserved);
    assert_eq!(executor.invocations, before);
}

#[test]
fn exactly_one_post_publication_record_reconciles_without_hardware() {
    let (catalog, mut session, store, executor, mut journal) = post_publication_crash();
    assert_eq!(store.records.len(), 1);
    assert_eq!(session.next_slot, 0);
    let invocations = executor.invocations;
    let anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&session);
    let result = journal
        .reconcile(
            &mut session,
            &store,
            &catalog,
            &anchors,
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    assert_eq!(
        result.observed_outcome,
        ReconciliationOutcome::ExactlyOneCompatiblePublication
    );
    assert_eq!(result.outcome, ReconciliationOutcome::Reconciled);
    assert_eq!(
        result.measurement_acquisition_reconciliation_hash,
        "805ab823e42c33a0822a836058a8ab43ba2759ce3f0f5f74ff1999ec49feb32a"
    );
    assert_eq!(session.next_slot, 1);
    assert_eq!(journal.status, RecoveryStatus::Complete);
    assert_eq!(executor.invocations, invocations);
}

#[test]
fn zero_publication_requires_separate_retry_authorization_and_new_attempt_id() {
    let (catalog, mut session, mut store, mut executor) = fixture(&["a"], 1);
    let mut journal = MeasurementAcquisitionRecoveryJournal::prepare(
        &session,
        &store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    journal
        .execute(
            &mut session,
            &mut store,
            &catalog,
            None,
            &mut executor,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::BeforeBenchmark),
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    let anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&session);
    journal
        .reconcile(
            &mut session,
            &store,
            &catalog,
            &anchors,
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    let old = journal.current_prepared_slot().unwrap().attempt_id.clone();
    let new = journal
        .authorize_retry(
            &session,
            &store,
            &catalog,
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap()
        .attempt_id
        .clone();
    assert_ne!(old, new);
    assert_eq!(journal.status, RecoveryStatus::RetryAuthorized);
    assert_eq!(journal.retry_authorizations.len(), 1);
}

#[test]
fn multiple_compatible_publications_remain_ambiguous() {
    let (catalog, mut session, mut store, mut executor) = fixture(&["a"], 1);
    let mut journal = MeasurementAcquisitionRecoveryJournal::prepare(
        &session,
        &store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    let slot = session.slots[0].clone();
    for _ in 0..2 {
        let (record, _) = executor
            .benchmark(None, &catalog, &session.plan, &session.preflight, &slot)
            .unwrap();
        store.publish(record).unwrap();
    }
    let anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&session);
    let result = journal
        .reconcile(
            &mut session,
            &store,
            &catalog,
            &anchors,
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    assert_eq!(
        result.outcome,
        ReconciliationOutcome::MultipleCompatiblePublications
    );
    assert_eq!(journal.status, RecoveryStatus::Ambiguous);
    assert_eq!(session.next_slot, 0);
}

#[test]
fn incompatible_publication_and_anchor_changes_are_typed_before_hardware() {
    let (catalog, mut session, mut store, mut executor) = fixture(&["a"], 1);
    let mut journal = MeasurementAcquisitionRecoveryJournal::prepare(
        &session,
        &store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    let slot = session.slots[0].clone();
    let (mut record, _) = executor
        .benchmark(None, &catalog, &session.plan, &session.preflight, &slot)
        .unwrap();
    record.runtime_version = "changed-runtime".to_owned();
    store.publish(record).unwrap();
    let anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&session);
    assert_eq!(
        journal
            .reconcile(
                &mut session,
                &store,
                &catalog,
                &anchors,
                &MeasurementAcquisitionRecoveryLimits::default(),
            )
            .unwrap()
            .outcome,
        ReconciliationOutcome::IncompatiblePublicationObserved
    );

    for (field, expected) in [
        ("device", ReconciliationOutcome::DeviceChanged),
        ("build", ReconciliationOutcome::BuildChanged),
        ("runtime", ReconciliationOutcome::RuntimeChanged),
        ("workspace", ReconciliationOutcome::WorkspaceChanged),
    ] {
        let (catalog, mut session, store, _, mut journal) = post_publication_crash();
        let mut current = MeasurementAcquisitionRecoveryAnchors::from_session(&session);
        match field {
            "device" => current.device_fingerprint_hash = "changed".to_owned(),
            "build" => current.compiler_build_hash = "changed".to_owned(),
            "runtime" => current.runtime_version = "changed".to_owned(),
            "workspace" => current.workspace_id = "changed".to_owned(),
            _ => unreachable!(),
        }
        let outcome = journal
            .reconcile(
                &mut session,
                &store,
                &catalog,
                &current,
                &MeasurementAcquisitionRecoveryLimits::default(),
            )
            .unwrap()
            .outcome;
        assert_eq!(outcome, expected);
    }
}

#[test]
fn corrupt_or_missing_measurements_reject_atomically_without_consuming_ids() {
    let (catalog, mut session, mut store, mut executor) = fixture(&["a"], 1);
    let slot = session.slots[0].clone();
    let (record, _) = executor
        .benchmark(None, &catalog, &session.plan, &session.preflight, &slot)
        .unwrap();
    let (baseline_id, _) = store.publish(record).unwrap();
    let mut journal = MeasurementAcquisitionRecoveryJournal::prepare(
        &session,
        &store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    store.records.get_mut(&baseline_id).unwrap().median_ns += 1;
    let before_journal = journal.clone();
    let before_session = session.clone();
    let before_store = store.clone();
    let anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&session);
    assert!(
        journal
            .reconcile(
                &mut session,
                &store,
                &catalog,
                &anchors,
                &MeasurementAcquisitionRecoveryLimits::default(),
            )
            .is_err()
    );
    assert_eq!(journal, before_journal);
    assert_eq!(session, before_session);
    assert_eq!(store, before_store);
}

#[test]
fn normal_and_crash_reconciled_stage7c_results_are_identical() {
    let (catalog, mut normal_session, mut normal_store, mut normal_executor) = fixture(&["a"], 1);
    let mut normal_journal = MeasurementAcquisitionRecoveryJournal::prepare(
        &normal_session,
        &normal_store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    normal_journal
        .execute(
            &mut normal_session,
            &mut normal_store,
            &catalog,
            None,
            &mut normal_executor,
            None,
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();

    let (catalog, mut recovered_session, recovered_store, _, mut recovered_journal) =
        post_publication_crash();
    let anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&recovered_session);
    recovered_journal
        .reconcile(
            &mut recovered_session,
            &recovered_store,
            &catalog,
            &anchors,
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    assert_eq!(
        normal_session.result().unwrap(),
        recovered_session.result().unwrap()
    );
}

#[test]
fn recovery_checkpoint_and_replay_have_zero_executor_calls() {
    let (catalog, session, store, executor, mut journal) = post_publication_crash();
    let checkpoint = journal
        .checkpoint(&session, &MeasurementAcquisitionRecoveryLimits::default())
        .unwrap();
    let restored = MeasurementAcquisitionRecoveryJournal::restore_checkpoint(
        &checkpoint,
        &session,
        &store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    let before = executor.invocations;
    let replay = restored
        .replay(
            &session,
            &store,
            &catalog,
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    assert_eq!(replay.work.replay_hardware_calls, 0);
    assert_eq!(executor.invocations, before);
}

#[test]
fn recovery_limits_are_exact_and_excluded_from_hashes() {
    let (catalog, session, store, _) = fixture(&["a"], 1);
    let exact = MeasurementAcquisitionRecoveryLimits {
        prepared_slots: 1,
        publication_snapshot_records: 0,
        recovery_trace_events: 1,
        ..MeasurementAcquisitionRecoveryLimits::default()
    };
    let wider = MeasurementAcquisitionRecoveryLimits::default();
    let left =
        MeasurementAcquisitionRecoveryJournal::prepare(&session, &store, &catalog, &exact).unwrap();
    let right =
        MeasurementAcquisitionRecoveryJournal::prepare(&session, &store, &catalog, &wider).unwrap();
    assert_eq!(
        left.measurement_acquisition_recovery_journal_hash,
        right.measurement_acquisition_recovery_journal_hash
    );
    let too_small = MeasurementAcquisitionRecoveryLimits {
        prepared_slots: 0,
        ..MeasurementAcquisitionRecoveryLimits::default()
    };
    assert_eq!(
        MeasurementAcquisitionRecoveryJournal::prepare(&session, &store, &catalog, &too_small,)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionLimitExceeded
    );
}

#[test]
fn v6_to_v7_migration_is_pure_and_v1_chain_reaches_v8() {
    let harness = EvaluationHarness::new().unwrap();
    let current = harness.archive(&[]).unwrap();
    assert_eq!(current.manifest.version, 8);
    let mut v6 = current.clone();
    v6.manifest.version = 6;
    v6.archive_hash.clear();
    v6.archive_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_V6_DOMAIN,
        &v6,
    )
    .unwrap();
    verify_archive(&v6).unwrap();
    let migrated = migrate_archive_v6_to_v7(&v6).unwrap();
    assert_eq!(migrated.manifest.version, 7);
    assert_eq!(
        migrated.measurement_acquisition_recovery_history_status,
        MeasurementAcquisitionRecoveryHistoryStatus::NoRecoveryHistory
    );
    assert_eq!(migrated.corpus, v6.corpus);
    assert_eq!(
        migrated.measurement_acquisition_results,
        v6.measurement_acquisition_results
    );

    let mut v1 = current;
    v1.manifest.version = 1;
    v1.feature_schemas.clear();
    v1.ranking_policies.clear();
    v1.choice_sets.clear();
    v1.ranking_statuses.clear();
    v1.learning_statuses.clear();
    v1.search_history_status = agentir_policy_eval::SearchHistoryStatus::Unspecified;
    v1.measured_search_history_status =
        agentir_policy_eval::MeasuredSearchHistoryStatus::Unspecified;
    v1.measurement_acquisition_history_status =
        agentir_policy_eval::MeasurementAcquisitionHistoryStatus::Unspecified;
    v1.archive_hash.clear();
    v1.archive_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_DOMAIN,
        &v1,
    )
    .unwrap();
    verify_archive(&v1).unwrap();
    let v2 = migrate_archive_v1_to_v2(&v1).unwrap();
    let v3 = migrate_archive_v2_to_v3(&v2).unwrap();
    let v4 = migrate_archive_v3_to_v4(&v3).unwrap();
    let v5 = migrate_archive_v4_to_v5(&v4).unwrap();
    let v6 = migrate_archive_v5_to_v6(&v5).unwrap();
    let v7 = migrate_archive_v6_to_v7(&v6).unwrap();
    let v8 = migrate_archive_v7_to_v8(&v7).unwrap();
    verify_archive(&v8).unwrap();
}

#[test]
fn archive_v7_round_trip_retains_recovery_and_rejects_mutation() {
    let (catalog, mut session, store, _, mut journal) = post_publication_crash();
    let anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&session);
    journal
        .reconcile(
            &mut session,
            &store,
            &catalog,
            &anchors,
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    let checkpoint = journal
        .checkpoint(&session, &MeasurementAcquisitionRecoveryLimits::default())
        .unwrap();
    let records = store
        .records
        .iter()
        .map(
            |(measurement_id, record)| agentir_policy_eval::MeasurementCohortRecord {
                measurement_id: measurement_id.clone(),
                record: record.clone(),
            },
        )
        .collect();
    let base = EvaluationHarness::new().unwrap().archive(&[]).unwrap();
    let archive = attach_measurement_acquisition_recovery_artifacts(
        &base,
        MeasurementAcquisitionRecoveryArchiveBundle {
            journals: vec![journal.clone()],
            checkpoints: vec![checkpoint],
            records,
            replay_statuses: BTreeMap::from([(
                journal
                    .measurement_acquisition_recovery_journal_hash
                    .clone(),
                true,
            )]),
        },
    )
    .unwrap();
    verify_archive(&archive).unwrap();
    let bytes = serde_json::to_vec(&archive).unwrap();
    let decoded = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(archive, decoded);
    let mut corrupt = archive;
    corrupt.measurement_acquisition_recovery_journals[0].slot_index += 1;
    corrupt.archive_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_V7_DOMAIN,
        &{
            let mut model = corrupt.clone();
            model.archive_hash.clear();
            model
        },
    )
    .unwrap();
    assert!(verify_archive(&corrupt).is_err());
}

#[test]
fn jsonl_recovery_commands_reject_client_metadata_and_execute_without_prepare() {
    let mut protocol =
        agentir_policy_eval::EvaluationProtocol::with_synthetic_acquisition_catalog(catalog())
            .unwrap();
    let forbidden: serde_json::Value = serde_json::from_str(&protocol.process_line(
        r#"{"command":"evaluation.measurement_acquisition.recovery.prepare","request_id":"bad","session":"missing","measurement_acquisition_plan_hash":"missing","median_ns":1,"device_fingerprint_hash":"client"}"#,
    ))
    .unwrap();
    assert_eq!(forbidden["ok"], false);
    let missing: serde_json::Value = serde_json::from_str(&protocol.process_line(
        r#"{"command":"evaluation.measurement_acquisition.recovery.execute","request_id":"bad","recovery_journal":"missing","measurement_acquisition_recovery_journal_hash":"missing"}"#,
    ))
    .unwrap();
    assert_eq!(missing["ok"], false);
    assert_eq!(
        missing["error"]["code"],
        "EVALUATION_ACQUISITION_RECOVERY_NOT_PREPARED"
    );
}

#[test]
fn jsonl_recovery_lifecycle_restarts_from_archive_and_reconciles_without_device() {
    fn send(
        protocol: &mut agentir_policy_eval::EvaluationProtocol,
        request: &serde_json::Value,
    ) -> serde_json::Value {
        serde_json::from_str(&protocol.process_line(&serde_json::to_string(request).unwrap()))
            .unwrap()
    }

    let mut protocol =
        agentir_policy_eval::EvaluationProtocol::with_synthetic_acquisition_catalog(catalog())
            .unwrap();
    let started = send(
        &mut protocol,
        &serde_json::json!({
            "command":"evaluation.measurement_acquisition.start",
            "request_id":"start",
            "task":"candidate-rewrite-small",
            "root_anchor_hash":"root-7d",
            "artifact_hashes":["a"],
            "benchmark_config":{
                "warmups":1,
                "iterations":3,
                "input_distribution":"deterministic_zero_v1",
                "tensor_dimensions":[4]
            },
            "records_per_artifact":1,
            "validation_policy":"synthetic_fixture_v1",
            "checkpoint_cadence_slots":1
        }),
    );
    assert_eq!(started["ok"], true);
    let session = started["result"]["session"].as_str().unwrap();
    let plan_hash = started["result"]["measurement_acquisition_plan_hash"]
        .as_str()
        .unwrap();
    let prepared = send(
        &mut protocol,
        &serde_json::json!({
            "command":"evaluation.measurement_acquisition.recovery.prepare",
            "request_id":"prepare",
            "session":session,
            "measurement_acquisition_plan_hash":plan_hash
        }),
    );
    let journal = prepared["result"]["recovery_journal"].as_str().unwrap();
    let prepared_hash = prepared["result"]["measurement_acquisition_recovery_journal_hash"]
        .as_str()
        .unwrap();
    let executed = send(
        &mut protocol,
        &serde_json::json!({
            "command":"evaluation.measurement_acquisition.recovery.execute",
            "request_id":"execute",
            "recovery_journal":journal,
            "measurement_acquisition_recovery_journal_hash":prepared_hash,
            "fault_boundary":"after_publication_before_checkpoint"
        }),
    );
    assert_eq!(executed["result"]["status"], "indeterminate_after_crash");
    let executed_hash = executed["result"]["measurement_acquisition_recovery_journal_hash"]
        .as_str()
        .unwrap();
    let checkpointed = send(
        &mut protocol,
        &serde_json::json!({
            "command":"evaluation.measurement_acquisition.recovery.checkpoint",
            "request_id":"checkpoint",
            "recovery_journal":journal,
            "measurement_acquisition_recovery_journal_hash":executed_hash
        }),
    );
    let checkpointed_hash = checkpointed["result"]["measurement_acquisition_recovery_journal_hash"]
        .as_str()
        .unwrap();
    let archive_path = std::env::temp_dir().join(format!(
        "agentir-stage7d-restart-{}.json",
        std::process::id()
    ));
    let saved = send(
        &mut protocol,
        &serde_json::json!({
            "command":"evaluation.archive.save",
            "request_id":"save",
            "path":archive_path,
            "runs":[]
        }),
    );
    assert_eq!(saved["ok"], true);

    let mut restarted =
        agentir_policy_eval::EvaluationProtocol::with_synthetic_acquisition_catalog(catalog())
            .unwrap();
    let loaded = send(
        &mut restarted,
        &serde_json::json!({
            "command":"evaluation.archive.load",
            "request_id":"load",
            "path":archive_path
        }),
    );
    assert_eq!(loaded["result"]["recovery_journals"], 1);
    let reconciled = send(
        &mut restarted,
        &serde_json::json!({
            "command":"evaluation.measurement_acquisition.recovery.reconcile",
            "request_id":"reconcile",
            "recovery_journal":journal,
            "measurement_acquisition_recovery_journal_hash":checkpointed_hash
        }),
    );
    assert_eq!(
        reconciled["result"]["reconciliation"]["outcome"],
        "reconciled"
    );
    assert_eq!(reconciled["result"]["hardware_calls"], 0);
    let reconciled_hash = reconciled["result"]["measurement_acquisition_recovery_journal_hash"]
        .as_str()
        .unwrap();
    let replayed = send(
        &mut restarted,
        &serde_json::json!({
            "command":"evaluation.measurement_acquisition.recovery.replay",
            "request_id":"replay",
            "recovery_journal":journal,
            "measurement_acquisition_recovery_journal_hash":reconciled_hash
        }),
    );
    assert_eq!(replayed["result"]["device_calls"], 0);
    std::fs::remove_file(archive_path).unwrap();
}

#[test]
fn abandon_is_explicit_and_completed_slots_cannot_reconcile_twice() {
    let (catalog, session, store, _) = fixture(&["a"], 1);
    let mut journal = MeasurementAcquisitionRecoveryJournal::prepare(
        &session,
        &store,
        &catalog,
        &MeasurementAcquisitionRecoveryLimits::default(),
    )
    .unwrap();
    assert_eq!(
        journal
            .abandon(&MeasurementAcquisitionRecoveryLimits::default())
            .unwrap(),
        RecoveryStatus::Abandoned
    );

    let (catalog, mut session, store, _, mut journal) = post_publication_crash();
    let anchors = MeasurementAcquisitionRecoveryAnchors::from_session(&session);
    journal
        .reconcile(
            &mut session,
            &store,
            &catalog,
            &anchors,
            &MeasurementAcquisitionRecoveryLimits::default(),
        )
        .unwrap();
    let before = journal.clone();
    assert!(
        journal
            .reconcile(
                &mut session,
                &store,
                &catalog,
                &anchors,
                &MeasurementAcquisitionRecoveryLimits::default(),
            )
            .is_err()
    );
    assert_eq!(journal, before);
}

#[test]
fn workspace_archive_advances_separately_while_measurement_contracts_remain_unchanged() {
    let registry: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/contract-registry.json")).unwrap();
    assert_eq!(registry["archive_families"][0]["current"], 11);
    let workspace_legacy = registry["archive_families"][0]["legacy"]
        .as_array()
        .unwrap();
    assert!(workspace_legacy.contains(&serde_json::json!(9)));
    assert!(workspace_legacy.contains(&serde_json::json!(10)));
    assert_eq!(agentir_core::backend_ir::MEASUREMENT_FORMAT_VERSION, 1);
    let (_, session, _, _) = fixture(&["a"], 1);
    assert_eq!(session.plan.version, 1);
    assert_eq!(
        agentir_core::backend::MEASUREMENT_HASH_DOMAIN,
        b"agentir.measurement.hardware.v1\0"
    );
}
