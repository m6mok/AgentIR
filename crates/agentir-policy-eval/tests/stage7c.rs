use agentir_core::{
    backend_ir::{ArtifactStatus, HardwareBenchmarkConfig},
    ids::ArtifactId,
};
use agentir_policy_eval::{
    EvaluationErrorCode, EvaluationHarness, EvaluationTaskId, MeasurementAcquisitionArchiveBundle,
    MeasurementAcquisitionArtifact, MeasurementAcquisitionCatalog, MeasurementAcquisitionExecutor,
    MeasurementAcquisitionFailurePolicy, MeasurementAcquisitionLimits,
    MeasurementAcquisitionOrderingPolicy, MeasurementAcquisitionPlan,
    MeasurementAcquisitionPlanRequest, MeasurementAcquisitionPreflight,
    MeasurementAcquisitionSession, MeasurementAcquisitionStatus, MeasurementValidationPolicy,
    SyntheticMeasurementAcquisitionExecutor, SyntheticMeasurementAcquisitionStore,
    attach_measurement_acquisition_artifacts, migrate_archive_v5_to_v6, verify_archive,
};

fn artifact(hash: &str) -> MeasurementAcquisitionArtifact {
    MeasurementAcquisitionArtifact {
        artifact_id: ArtifactId::new(format!("artifact-{hash}")),
        artifact_hash: hash.to_owned(),
        spec_hash: "spec-1".to_owned(),
        target_hash: "target-1".to_owned(),
        compiler_build_hash: "build-1".to_owned(),
        status: ArtifactStatus::Validated,
        offline_valid: true,
    }
}

fn request(hashes: &[&str], records: u64) -> MeasurementAcquisitionPlanRequest {
    MeasurementAcquisitionPlanRequest {
        corpus_hash: "corpus-1".to_owned(),
        task_id: EvaluationTaskId("task-1".to_owned()),
        root_anchor_hash: "root-1".to_owned(),
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

fn catalog() -> MeasurementAcquisitionCatalog {
    MeasurementAcquisitionCatalog::synthetic_fixture(
        "workspace-1".to_owned(),
        "root-1".to_owned(),
        vec![artifact("c"), artifact("a"), artifact("b")],
    )
    .unwrap()
}

#[test]
fn plan_is_canonical_and_round_robin() {
    let catalog = catalog();
    let left = MeasurementAcquisitionPlan::new(&catalog, request(&["c", "a", "b"], 3)).unwrap();
    let right = MeasurementAcquisitionPlan::new(&catalog, request(&["b", "c", "a"], 3)).unwrap();
    assert_eq!(left, right);
    assert_eq!(left.artifact_hashes, ["a", "b", "c"]);
    assert_eq!(
        left.slots()
            .unwrap()
            .into_iter()
            .map(|slot| (slot.slot_index, slot.round_index, slot.artifact_hash))
            .collect::<Vec<_>>(),
        vec![
            (0, 0, "a".to_owned()),
            (1, 0, "b".to_owned()),
            (2, 0, "c".to_owned()),
            (3, 1, "a".to_owned()),
            (4, 1, "b".to_owned()),
            (5, 1, "c".to_owned()),
            (6, 2, "a".to_owned()),
            (7, 2, "b".to_owned()),
            (8, 2, "c".to_owned()),
        ]
    );
    left.verify().unwrap();
}

#[test]
fn plan_boundaries_and_mixed_anchors_reject() {
    let catalog = catalog();
    assert_eq!(
        MeasurementAcquisitionPlan::new(&catalog, request(&[], 1))
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionPlanInvalid
    );
    let mut mixed = catalog.clone();
    mixed.artifacts.get_mut("b").unwrap().spec_hash = "spec-2".to_owned();
    assert_eq!(
        MeasurementAcquisitionPlan::new(&mixed, request(&["a", "b"], 1))
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionMixedSpec
    );
    let mut mixed_target = catalog.clone();
    mixed_target.artifacts.get_mut("b").unwrap().target_hash = "target-2".to_owned();
    assert_eq!(
        MeasurementAcquisitionPlan::new(&mixed_target, request(&["a", "b"], 1))
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionMixedTarget
    );
    let mut invalid = catalog.clone();
    invalid.artifacts.get_mut("a").unwrap().offline_valid = false;
    assert_eq!(
        MeasurementAcquisitionPlan::new(&invalid, request(&["a"], 1))
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionArtifactInvalid
    );
    assert_eq!(
        MeasurementAcquisitionPlan::new(&catalog, request(&["missing"], 1))
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionArtifactSetInvalid
    );
    let mut hardware = request(&["a"], 1);
    hardware.validation_policy = MeasurementValidationPolicy::HardwareExecutedV1;
    assert_eq!(
        MeasurementAcquisitionPlan::new(&catalog, hardware)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionUnsupportedMode
    );
}

#[test]
fn plan_operational_limits_are_exact_and_hash_independent() {
    let catalog = catalog();
    let exact = MeasurementAcquisitionLimits {
        artifact_references: 3,
        records_per_artifact: 2,
        total_slots: 6,
        checkpoint_bytes: u64::MAX,
    };
    let wider = MeasurementAcquisitionLimits {
        artifact_references: 4,
        records_per_artifact: 3,
        total_slots: 12,
        checkpoint_bytes: u64::MAX,
    };
    let plan =
        MeasurementAcquisitionPlan::new_with_limits(&catalog, request(&["a", "b", "c"], 2), &exact)
            .unwrap();
    let same =
        MeasurementAcquisitionPlan::new_with_limits(&catalog, request(&["a", "b", "c"], 2), &wider)
            .unwrap();
    assert_eq!(plan, same);
    assert_eq!(plan.total_slots().unwrap(), 6);
    assert_eq!(
        MeasurementAcquisitionPlan::new_with_limits(
            &catalog,
            request(&["a", "b", "c"], 3),
            &exact,
        )
        .unwrap_err()
        .code,
        EvaluationErrorCode::EvaluationAcquisitionLimitExceeded
    );
    for hashes in [&["a"][..], &["a", "b"][..], &["a", "b", "c"][..]] {
        MeasurementAcquisitionPlan::new(&catalog, request(hashes, 2)).unwrap();
    }
}

#[test]
fn plan_hash_is_stable_and_domain_separated() {
    let plan = MeasurementAcquisitionPlan::new(&catalog(), request(&["a", "b", "c"], 3)).unwrap();
    assert_eq!(
        plan.measurement_acquisition_plan_hash,
        "8383cffe6fc500bfc27ea599cfcb3f8903bfa6af62b6518240d4ba16415b7cda"
    );
    assert_ne!(
        agentir_policy_eval::MEASUREMENT_ACQUISITION_PLAN_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_COHORT_HASH_DOMAIN
    );
}

fn completed_with_chunks(
    chunks: &[u64],
) -> (
    agentir_policy_eval::MeasurementAcquisitionResult,
    SyntheticMeasurementAcquisitionStore,
) {
    let catalog = catalog();
    let plan = MeasurementAcquisitionPlan::new(&catalog, request(&["a", "b", "c"], 3)).unwrap();
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let mut session =
        MeasurementAcquisitionSession::start(plan, &catalog, None, &mut executor).unwrap();
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    for chunk in chunks {
        session
            .advance(&mut store, &catalog, None, &mut executor, *chunk)
            .unwrap();
    }
    if session.status == MeasurementAcquisitionStatus::Running {
        session
            .advance(&mut store, &catalog, None, &mut executor, u64::MAX)
            .unwrap();
    }
    assert_eq!(session.status, MeasurementAcquisitionStatus::Complete);
    (session.result().unwrap(), store)
}

#[test]
fn single_shot_and_split_execution_are_identical() {
    let (single, _) = completed_with_chunks(&[u64::MAX]);
    let (split, _) = completed_with_chunks(&[1, 2, 1, 3, 2]);
    assert_eq!(single, split);
    assert_eq!(single.measurement_ids.len(), 9);
    assert_eq!(single.completed_records_per_artifact["a"], 3);
}

#[test]
fn checkpoint_resume_skips_completed_slots_and_replay_calls_no_executor() {
    let catalog = catalog();
    let plan = MeasurementAcquisitionPlan::new(&catalog, request(&["a", "b", "c"], 3)).unwrap();
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let mut session =
        MeasurementAcquisitionSession::start(plan, &catalog, None, &mut executor).unwrap();
    let preflight = session.preflight.clone();
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    session
        .advance(&mut store, &catalog, None, &mut executor, 4)
        .unwrap();
    let checkpoint = session.checkpoint().unwrap();
    let before_resume_invocations = executor.invocations;
    let mut resumed =
        MeasurementAcquisitionSession::resume(&checkpoint, &store, &catalog, &preflight).unwrap();
    assert_eq!(executor.invocations, before_resume_invocations);
    resumed
        .advance(&mut store, &catalog, None, &mut executor, u64::MAX)
        .unwrap();
    let before_replay = executor.invocations;
    let result = resumed.result().unwrap();
    let replayed = resumed.replay(&store, &catalog).unwrap();
    assert_eq!(executor.invocations, before_replay);
    assert_eq!(result, replayed);
    assert_eq!(resumed.work.prevented_reruns, 4);
}

#[test]
fn resume_rejects_corrupt_missing_duplicate_and_changed_anchors_before_hardware() {
    let catalog = catalog();
    let plan = MeasurementAcquisitionPlan::new(&catalog, request(&["a", "b"], 1)).unwrap();
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let mut session =
        MeasurementAcquisitionSession::start(plan, &catalog, None, &mut executor).unwrap();
    let preflight = session.preflight.clone();
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    session
        .advance(&mut store, &catalog, None, &mut executor, 1)
        .unwrap();
    let checkpoint = session.checkpoint().unwrap();

    let mut corrupt = checkpoint.clone();
    corrupt.completed_slot_indices.clear();
    corrupt.measurement_acquisition_checkpoint_hash =
        agentir_policy_eval::measurement_acquisition_checkpoint_hash(&corrupt).unwrap();
    assert_eq!(
        MeasurementAcquisitionSession::resume(&corrupt, &store, &catalog, &preflight)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionCheckpointCorrupt
    );

    let empty_store = SyntheticMeasurementAcquisitionStore::default();
    assert_eq!(
        MeasurementAcquisitionSession::resume(&checkpoint, &empty_store, &catalog, &preflight)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionMeasurementMissing
    );

    let duplicate_plan =
        MeasurementAcquisitionPlan::new(&catalog, request(&["a", "b"], 1)).unwrap();
    let mut duplicate_session =
        MeasurementAcquisitionSession::start(duplicate_plan, &catalog, None, &mut executor)
            .unwrap();
    let duplicate_preflight = duplicate_session.preflight.clone();
    let mut duplicate_store = SyntheticMeasurementAcquisitionStore::default();
    duplicate_session
        .advance(
            &mut duplicate_store,
            &catalog,
            None,
            &mut executor,
            u64::MAX,
        )
        .unwrap();
    duplicate_session.slots[1].measurement_id = duplicate_session.slots[0].measurement_id.clone();
    duplicate_session.slots[1].measurement_hash =
        duplicate_session.slots[0].measurement_hash.clone();
    let duplicate_checkpoint = duplicate_session.checkpoint().unwrap();
    assert_eq!(
        MeasurementAcquisitionSession::resume(
            &duplicate_checkpoint,
            &duplicate_store,
            &catalog,
            &duplicate_preflight,
        )
        .unwrap_err()
        .code,
        EvaluationErrorCode::EvaluationAcquisitionMeasurementDuplicate
    );

    let invocations = executor.invocations;
    for (field, expected) in [
        (
            "device",
            EvaluationErrorCode::EvaluationAcquisitionDeviceChanged,
        ),
        (
            "build",
            EvaluationErrorCode::EvaluationAcquisitionCompilerBuildChanged,
        ),
        (
            "runtime",
            EvaluationErrorCode::EvaluationAcquisitionRuntimeChanged,
        ),
    ] {
        let mut changed = preflight.clone();
        match field {
            "device" => changed.device_fingerprint_hash = "changed".to_owned(),
            "build" => changed.compiler_build_hash = "changed".to_owned(),
            "runtime" => changed.runtime_version = "changed".to_owned(),
            _ => unreachable!(),
        }
        assert_eq!(
            MeasurementAcquisitionSession::resume(&checkpoint, &store, &catalog, &changed)
                .unwrap_err()
                .code,
            expected
        );
    }
    assert_eq!(executor.invocations, invocations);
}

#[test]
fn crash_ambiguity_is_typed_and_checkpoint_bytes_are_bounded() {
    let catalog = catalog();
    let plan = MeasurementAcquisitionPlan::new(&catalog, request(&["a"], 1)).unwrap();
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let mut session =
        MeasurementAcquisitionSession::start(plan, &catalog, None, &mut executor).unwrap();
    session.mark_indeterminate_after_crash().unwrap();
    assert_eq!(session.status, MeasurementAcquisitionStatus::Failed);
    assert_eq!(session.work.slots_indeterminate, 1);
    assert_eq!(
        session.slots[0].failure_code,
        Some(EvaluationErrorCode::EvaluationAcquisitionIndeterminateAfterCrash)
    );
    let result = session.result().unwrap();
    assert_eq!(
        result.stopping_reason,
        agentir_policy_eval::MeasurementAcquisitionStoppingReason::IndeterminateAfterCrash
    );
    let limits = MeasurementAcquisitionLimits {
        checkpoint_bytes: 1,
        ..MeasurementAcquisitionLimits::default()
    };
    assert_eq!(
        session.checkpoint_with_limits(&limits).unwrap_err().code,
        EvaluationErrorCode::EvaluationAcquisitionLimitExceeded
    );
}

#[derive(Clone)]
struct InvalidRecordExecutor(SyntheticMeasurementAcquisitionExecutor);

impl MeasurementAcquisitionExecutor for InvalidRecordExecutor {
    fn preflight(
        &mut self,
        workspace: Option<&agentir_core::Workspace>,
        catalog: &MeasurementAcquisitionCatalog,
        plan: &MeasurementAcquisitionPlan,
    ) -> agentir_policy_eval::EvaluationResult<MeasurementAcquisitionPreflight> {
        self.0.preflight(workspace, catalog, plan)
    }

    fn benchmark(
        &mut self,
        workspace: Option<&agentir_core::Workspace>,
        catalog: &MeasurementAcquisitionCatalog,
        plan: &MeasurementAcquisitionPlan,
        preflight: &MeasurementAcquisitionPreflight,
        slot: &agentir_policy_eval::MeasurementAcquisitionSlot,
    ) -> agentir_policy_eval::EvaluationResult<(
        agentir_core::backend_ir::HardwareMeasurementRecord,
        u64,
    )> {
        let (mut record, calls) = self
            .0
            .benchmark(workspace, catalog, plan, preflight, slot)?;
        "client-shaped-invalid-runtime".clone_into(&mut record.runtime_version);
        Ok((record, calls))
    }
}

#[test]
fn rejected_advance_is_atomic_for_session_and_store() {
    let catalog = catalog();
    let plan = MeasurementAcquisitionPlan::new(&catalog, request(&["a"], 1)).unwrap();
    let mut executor = InvalidRecordExecutor(SyntheticMeasurementAcquisitionExecutor::new());
    let mut session =
        MeasurementAcquisitionSession::start(plan, &catalog, None, &mut executor).unwrap();
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    let before_session = session.clone();
    let before_store = store.clone();
    assert_eq!(
        session
            .advance(&mut store, &catalog, None, &mut executor, 1)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionBenchmarkFailed
    );
    assert_eq!(session, before_session);
    assert_eq!(store, before_store);
}

#[test]
fn cancellation_and_benchmark_failure_publish_no_partial_record() {
    let catalog = catalog();
    let plan = MeasurementAcquisitionPlan::new(&catalog, request(&["a", "b"], 2)).unwrap();
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    executor.fail_slot = Some(1);
    let mut session =
        MeasurementAcquisitionSession::start(plan, &catalog, None, &mut executor).unwrap();
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    session
        .advance(&mut store, &catalog, None, &mut executor, u64::MAX)
        .unwrap();
    assert_eq!(
        session.status,
        MeasurementAcquisitionStatus::BenchmarkFailed
    );
    assert_eq!(store.records.len(), 1);
    assert!(session.slots[1].measurement_id.is_none());
    assert!(session.slots[1].measurement_hash.is_none());
    assert_eq!(
        session.slots[1].failure_code,
        Some(EvaluationErrorCode::EvaluationAcquisitionBenchmarkFailed)
    );

    let plan = MeasurementAcquisitionPlan::new(&catalog, request(&["a"], 1)).unwrap();
    let mut cancelled =
        MeasurementAcquisitionSession::start(plan, &catalog, None, &mut executor).unwrap();
    cancelled.cancel().unwrap();
    assert_eq!(cancelled.status, MeasurementAcquisitionStatus::Cancelled);
}

#[test]
fn archive_v5_to_v6_migration_is_pure_and_v6_replays_acquisition() {
    let harness = EvaluationHarness::new().unwrap();
    let mut legacy = harness.archive(&[]).unwrap();
    legacy.manifest.version = 5;
    legacy.measurement_acquisition_history_status =
        agentir_policy_eval::MeasurementAcquisitionHistoryStatus::Unspecified;
    legacy.archive_hash.clear();
    legacy.archive_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_V5_DOMAIN,
        &legacy,
    )
    .unwrap();
    verify_archive(&legacy).unwrap();
    let migrated = migrate_archive_v5_to_v6(&legacy).unwrap();
    assert_eq!(migrated.manifest.version, 6);
    assert_eq!(
        migrated.measurement_acquisition_history_status,
        agentir_policy_eval::MeasurementAcquisitionHistoryStatus::NoAcquisitionHistory
    );

    let catalog = catalog();
    let plan = MeasurementAcquisitionPlan::new(&catalog, request(&["a", "b"], 1)).unwrap();
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let mut session =
        MeasurementAcquisitionSession::start(plan, &catalog, None, &mut executor).unwrap();
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    session
        .advance(&mut store, &catalog, None, &mut executor, u64::MAX)
        .unwrap();
    let checkpoint = session.checkpoint().unwrap();
    let records = store
        .records
        .iter()
        .map(
            |(id, record)| agentir_policy_eval::MeasurementCohortRecord {
                measurement_id: id.clone(),
                record: record.clone(),
            },
        )
        .collect();
    let archive = attach_measurement_acquisition_artifacts(
        &migrated,
        MeasurementAcquisitionArchiveBundle {
            sessions: vec![session],
            checkpoints: vec![checkpoint],
            records,
            cohort_links: std::collections::BTreeMap::default(),
        },
    )
    .unwrap();
    verify_archive(&archive).unwrap();
    assert_eq!(archive.measurement_acquisition_results.len(), 1);
}

#[test]
fn jsonl_acquisition_lifecycle_and_trust_boundary() {
    let mut protocol =
        agentir_policy_eval::EvaluationProtocol::with_synthetic_acquisition_catalog(catalog())
            .unwrap();
    let forbidden: serde_json::Value = serde_json::from_str(&protocol.process_line(
        r#"{"command":"evaluation.measurement_acquisition.start","request_id":"bad","task":"candidate-rewrite-small","root_anchor_hash":"root-1","artifact_hashes":["a"],"benchmark_config":{"warmups":1,"iterations":3,"input_distribution":"deterministic_zero_v1","tensor_dimensions":[4]},"records_per_artifact":1,"validation_policy":"synthetic_fixture_v1","checkpoint_cadence_slots":1,"median_ns":1,"device_fingerprint_hash":"client-forbidden"}"#,
    ))
    .unwrap();
    assert_eq!(forbidden["ok"], false);

    let started: serde_json::Value = serde_json::from_str(&protocol.process_line(
        r#"{"command":"evaluation.measurement_acquisition.start","request_id":"s","task":"candidate-rewrite-small","root_anchor_hash":"root-1","artifact_hashes":["b","a"],"benchmark_config":{"warmups":1,"iterations":3,"input_distribution":"deterministic_zero_v1","tensor_dimensions":[4]},"records_per_artifact":2,"validation_policy":"synthetic_fixture_v1","checkpoint_cadence_slots":1}"#,
    ))
    .unwrap();
    assert_eq!(started["ok"], true);
    let session = started["result"]["session"].as_str().unwrap();
    let plan = started["result"]["measurement_acquisition_plan_hash"]
        .as_str()
        .unwrap();
    let advanced: serde_json::Value = serde_json::from_str(&protocol.process_line(&format!(
        "{{\"command\":\"evaluation.measurement_acquisition.advance\",\"request_id\":\"a\",\"session\":\"{session}\",\"measurement_acquisition_plan_hash\":\"{plan}\",\"maximum_slots\":99}}"
    )))
    .unwrap();
    assert_eq!(advanced["result"]["status"], "complete");
    let result: serde_json::Value = serde_json::from_str(&protocol.process_line(&format!(
        "{{\"command\":\"evaluation.measurement_acquisition.result\",\"request_id\":\"r\",\"session\":\"{session}\",\"measurement_acquisition_plan_hash\":\"{plan}\"}}"
    )))
    .unwrap();
    let result_hash = result["result"]["measurement_acquisition_result_hash"]
        .as_str()
        .unwrap();
    let replay: serde_json::Value = serde_json::from_str(&protocol.process_line(&format!(
        "{{\"command\":\"evaluation.measurement_acquisition.replay\",\"request_id\":\"p\",\"session\":\"{session}\",\"measurement_acquisition_result_hash\":\"{result_hash}\"}}"
    )))
    .unwrap();
    assert_eq!(replay["result"]["device_calls"], 0);
    let cohort: serde_json::Value = serde_json::from_str(&protocol.process_line(&format!(
        "{{\"command\":\"evaluation.measurement_acquisition.create_cohort\",\"request_id\":\"c\",\"session\":\"{session}\",\"measurement_acquisition_result_hash\":\"{result_hash}\",\"aggregation_method\":\"median_of_record_summaries_v1\"}}"
    )))
    .unwrap();
    assert_eq!(cohort["ok"], true);
}

#[test]
fn checkpoint_trace_and_result_hashes_are_stable_and_separate() {
    let catalog = catalog();
    let plan = MeasurementAcquisitionPlan::new(&catalog, request(&["a"], 1)).unwrap();
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let mut session =
        MeasurementAcquisitionSession::start(plan, &catalog, None, &mut executor).unwrap();
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    session
        .advance(&mut store, &catalog, None, &mut executor, 1)
        .unwrap();
    let checkpoint = session.checkpoint().unwrap();
    let result = session.result().unwrap();
    assert_eq!(
        checkpoint.measurement_acquisition_checkpoint_hash,
        "f92b9da9667f241a4bf1a3b59b3348f1973dcc0e507596a84c7d33c5eec12ed5"
    );
    assert_eq!(
        session.trace.measurement_acquisition_trace_hash,
        "1e1ea20e04e2ff2980c585eae3932735ef5831c220d56e3861cf2be8816206e3"
    );
    assert_eq!(
        result.measurement_acquisition_result_hash,
        "928e3399048e9374b64fa63378fcc2d0d7e037447c8f054d002e458a7a8445c3"
    );
    let domains = [
        agentir_policy_eval::MEASUREMENT_ACQUISITION_PLAN_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_ACQUISITION_CHECKPOINT_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_ACQUISITION_TRACE_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_ACQUISITION_RESULT_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_COHORT_HASH_DOMAIN,
    ];
    assert_eq!(
        domains
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        5
    );
}
