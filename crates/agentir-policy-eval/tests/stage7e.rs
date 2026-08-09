use agentir_core::{backend_ir::HardwareBenchmarkConfig, ids::ArtifactId};
use agentir_policy_eval::ranking::{feature_schema_v1, scripted_ranker};
use agentir_policy_eval::{
    AutotuningCampaignArchiveBundle, AutotuningCampaignLimits, AutotuningCampaignPlan,
    AutotuningCampaignSession, AutotuningCampaignStatus, EvaluationErrorCode, EvaluationHarness,
    EvaluationLimits, MeasuredMetric, MeasurementAcquisitionArtifact,
    MeasurementAcquisitionCatalog, MeasurementAcquisitionRecoveryFaultBoundary,
    MeasurementAcquisitionRecoveryLimits, MeasurementAggregationMethod,
    MeasurementValidationPolicy, ObjectiveDirection, SearchLimits, SearchObjectiveComponent,
    SearchObjectiveComponentKind, SearchObjectiveDescriptor, SearchPlan, SearchRanker,
    SearchSession, SyntheticMeasurementAcquisitionExecutor, SyntheticMeasurementAcquisitionStore,
    attach_autotuning_campaign_artifacts, autotuning_campaign_checkpoint_hash,
    autotuning_campaign_plan_hash, autotuning_campaign_result_hash,
    autotuning_campaign_session_hash, autotuning_campaign_trace_hash, builtin_ranked_corpus,
    migrate_archive_v7_to_v8, terminal_artifact_hash, verify_archive,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn corpus() -> agentir_policy_eval::EvaluationCorpus {
    let mut corpus = builtin_ranked_corpus().unwrap();
    let requests = include_str!("../../../examples/stage7e_two_artifact.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect::<Vec<Value>>();
    let task = corpus
        .tasks
        .iter_mut()
        .find(|task| task.id.0 == "ranked-backend-large")
        .unwrap();
    task.initial_state.production_requests = requests;
    task.budget.max_actions = u64::try_from(task.initial_state.production_requests.len())
        .unwrap()
        .saturating_add(4);
    task.budget.max_rejections = 4;
    "materialize two production-replayed terminal artifacts"
        .clone_into(&mut task.objective.summary);
    task.metadata.insert(
        "stage7e_fixture".to_owned(),
        "synthetic_test_data_not_performance_evidence".to_owned(),
    );
    "stage7e-campaign-tests".clone_into(&mut corpus.name);
    "stage7e-tests-v1".clone_into(&mut corpus.version);
    corpus.corpus_hash.clear();
    corpus.corpus_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::CORPUS_HASH_DOMAIN,
        &corpus,
    )
    .unwrap();
    corpus
}

fn planned_campaign() -> AutotuningCampaignSession {
    let corpus = corpus();
    let task = corpus
        .tasks
        .iter()
        .find(|task| task.id.0 == "ranked-backend-large")
        .unwrap()
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
    .unwrap();
    let ranker = SearchRanker::Scripted {
        descriptor: scripted_ranker("goal_directed_rule_v1", &feature_schema_v1().unwrap(), 0)
            .unwrap(),
    };
    let search_plan = SearchPlan::deterministic_beam_v1(&objective, &ranker, 4, 64, 4, 1).unwrap();
    let search = SearchSession::start(corpus, task.id, objective, search_plan, &ranker).unwrap();
    let plan = AutotuningCampaignPlan::new(
        &search,
        &ranker,
        HardwareBenchmarkConfig {
            warmups: 2,
            iterations: 5,
            input_distribution: "deterministic_zero_v1".to_owned(),
            tensor_dimensions: vec![4],
        },
        1,
        MeasurementValidationPolicy::SyntheticFixtureV1,
        1,
        MeasuredMetric::MedianNs,
        MeasurementAggregationMethod::SingleRecordSummaryV1,
        0,
        32,
    )
    .unwrap();
    AutotuningCampaignSession::start(search, ranker, plan, &AutotuningCampaignLimits::default())
        .unwrap()
}

fn search_complete() -> AutotuningCampaignSession {
    let mut campaign = planned_campaign();
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .advance_search(
            &base,
            u64::MAX,
            &SearchLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    assert_eq!(campaign.status, AutotuningCampaignStatus::SearchComplete);
    campaign
}

fn catalog(campaign: &AutotuningCampaignSession) -> MeasurementAcquisitionCatalog {
    let artifacts = campaign
        .search
        .nodes
        .values()
        .filter(|node| node.terminal)
        .filter_map(|node| production_terminal_artifact(campaign, &node.id))
        .collect::<Vec<_>>();
    assert!(artifacts.len() >= 2);
    MeasurementAcquisitionCatalog::synthetic_fixture(
        "stage7e-production-replayed-terminal-paths".to_owned(),
        campaign.plan.initial_anchor_hash.clone(),
        artifacts,
    )
    .unwrap()
}

fn production_terminal_artifact(
    campaign: &AutotuningCampaignSession,
    terminal: &agentir_policy_eval::SearchNodeId,
) -> Option<MeasurementAcquisitionArtifact> {
    let artifact_hash = terminal_artifact_hash(&campaign.search, terminal).unwrap()?;
    let mut node = campaign.search.nodes.get(terminal).unwrap();
    let mut spec_hashes = BTreeSet::new();
    let mut target_hashes = BTreeSet::new();
    let mut publication = None;
    while let Some(edge_id) = &node.parent_edge {
        let edge = campaign.search.edges.get(edge_id).unwrap();
        collect_named_string(
            &edge.compiler_outcome.response,
            "spec_hash",
            &mut spec_hashes,
        );
        collect_named_string(
            &edge.compiler_outcome.response,
            "target_hash",
            &mut target_hashes,
        );
        let result = edge.compiler_outcome.response.get("result");
        if result
            .and_then(|value| value.pointer("/query/artifact_hash"))
            .and_then(Value::as_str)
            == Some(artifact_hash.as_str())
        {
            let result = result.unwrap();
            assert!(edge.compiler_outcome.accepted);
            assert_eq!(
                result.get("equivalent_to_backend"),
                Some(&Value::Bool(true))
            );
            assert_eq!(result.get("offline_valid"), Some(&Value::Bool(true)));
            let query = result.get("query").unwrap();
            let status = match query.get("status").and_then(Value::as_str) {
                Some("validated") => agentir_core::backend_ir::ArtifactStatus::Validated,
                Some("sealed") => agentir_core::backend_ir::ArtifactStatus::Sealed,
                other => panic!("unexpected terminal artifact status: {other:?}"),
            };
            publication = Some((
                ArtifactId::new(query.get("artifact").and_then(Value::as_str).unwrap()),
                query
                    .get("compiler_build_hash")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_owned(),
                status,
            ));
        }
        node = campaign.search.nodes.get(&edge.parent).unwrap();
    }
    let (artifact_id, compiler_build_hash, status) = publication.unwrap();
    assert_eq!(spec_hashes.len(), 1);
    assert_eq!(target_hashes.len(), 1);
    Some(MeasurementAcquisitionArtifact {
        artifact_id,
        artifact_hash,
        spec_hash: spec_hashes.into_iter().next().unwrap(),
        target_hash: target_hashes.into_iter().next().unwrap(),
        compiler_build_hash,
        status,
        offline_valid: true,
    })
}

fn collect_named_string(value: &Value, name: &str, output: &mut BTreeSet<String>) {
    match value {
        Value::Object(fields) => {
            if let Some(value) = fields.get(name).and_then(Value::as_str) {
                output.insert(value.to_owned());
            }
            for value in fields.values() {
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

fn complete_campaign() -> (
    AutotuningCampaignSession,
    MeasurementAcquisitionCatalog,
    SyntheticMeasurementAcquisitionStore,
    SyntheticMeasurementAcquisitionExecutor,
) {
    let mut campaign = search_complete();
    let catalog = catalog(&campaign);
    let mut base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .prepare_acquisition(
            &base,
            &catalog,
            &agentir_policy_eval::MeasurementAcquisitionLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    while campaign.status != AutotuningCampaignStatus::AcquisitionComplete {
        base.clone_from(&campaign.autotuning_campaign_session_hash);
        campaign
            .execute_prepared(
                &base,
                &mut store,
                &catalog,
                None,
                &mut executor,
                None,
                &MeasurementAcquisitionRecoveryLimits::default(),
                &AutotuningCampaignLimits::default(),
            )
            .unwrap();
    }
    base.clone_from(&campaign.autotuning_campaign_session_hash);
    campaign
        .create_cohort(&base, &store, &AutotuningCampaignLimits::default())
        .unwrap();
    base.clone_from(&campaign.autotuning_campaign_session_hash);
    campaign
        .recommend(&base, &AutotuningCampaignLimits::default())
        .unwrap();
    base.clone_from(&campaign.autotuning_campaign_session_hash);
    campaign
        .finalize(&base, &AutotuningCampaignLimits::default())
        .unwrap();
    (campaign, catalog, store, executor)
}

fn resume_boundary(
    mut campaign: AutotuningCampaignSession,
    store: &SyntheticMeasurementAcquisitionStore,
    catalog: Option<&MeasurementAcquisitionCatalog>,
) -> AutotuningCampaignSession {
    let status = campaign.status;
    let semantic_hash = campaign.autotuning_campaign_session_hash.clone();
    let base = semantic_hash.clone();
    let checkpoint = campaign
        .checkpoint(
            &base,
            &SearchLimits::default(),
            &MeasurementAcquisitionRecoveryLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    let resumed = AutotuningCampaignSession::resume(
        &checkpoint,
        store,
        catalog,
        &SearchLimits::default(),
        &MeasurementAcquisitionRecoveryLimits::default(),
        &AutotuningCampaignLimits::default(),
    )
    .unwrap();
    assert_eq!(resumed.status, status);
    assert_eq!(resumed.autotuning_campaign_session_hash, semantic_hash);
    resumed
}

#[test]
fn campaign_hashes_are_stable_and_domain_separated() {
    let campaign = planned_campaign();
    assert_eq!(
        campaign.plan.autotuning_campaign_plan_hash,
        "8b64b3e9e5407bd231690f3aabcf8ec67b561e98f77388a29bdee3838a372e0e"
    );
    assert_eq!(
        campaign.autotuning_campaign_session_hash,
        "d30692f1c72e962ee40b32048c7a34d3adf297f50d89cdde3532f10b40f6b72d"
    );
    assert_eq!(
        campaign.trace.autotuning_campaign_trace_hash,
        "73a5648caf085a51015b33d6cfe64fa6f7d30aada749f690a18b46e5da9985a7"
    );
    assert_eq!(
        campaign.plan.autotuning_campaign_plan_hash,
        autotuning_campaign_plan_hash(&campaign.plan).unwrap()
    );
    assert_eq!(
        campaign.autotuning_campaign_session_hash,
        autotuning_campaign_session_hash(&campaign).unwrap()
    );
    assert_eq!(
        campaign.trace.autotuning_campaign_trace_hash,
        autotuning_campaign_trace_hash(&campaign.trace).unwrap()
    );
    let hashes = BTreeSet::from([
        campaign.plan.autotuning_campaign_plan_hash.clone(),
        campaign.autotuning_campaign_session_hash.clone(),
        campaign.trace.autotuning_campaign_trace_hash.clone(),
        campaign.search.plan.search_plan_hash.clone(),
        campaign.search.objective.search_objective_hash.clone(),
    ]);
    assert_eq!(hashes.len(), 5);
}

#[test]
fn start_search_and_rejected_transitions_are_zero_device_and_atomic() {
    let mut campaign = planned_campaign();
    assert_eq!(campaign.work.hardware_calls, 0);
    let before = campaign.clone();
    let stale = campaign
        .advance_search(
            "stale",
            1,
            &SearchLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap_err();
    assert_eq!(
        stale.code,
        EvaluationErrorCode::EvaluationAutotuningCampaignAnchorStale
    );
    assert_eq!(campaign, before);
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .advance_search(
            &base,
            u64::MAX,
            &SearchLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    assert_eq!(campaign.work.hardware_calls, 0);
}

#[test]
fn integrated_search_materializes_at_least_two_production_terminal_artifacts() {
    let mut campaign = search_complete();
    let terminal_hashes = campaign
        .search
        .nodes
        .values()
        .filter(|node| node.terminal)
        .filter_map(|node| terminal_artifact_hash(&campaign.search, &node.id).unwrap())
        .collect::<BTreeSet<_>>();
    assert!(terminal_hashes.len() >= 2);
    let catalog = catalog(&campaign);
    assert_eq!(
        catalog.artifacts.keys().cloned().collect::<BTreeSet<_>>(),
        terminal_hashes
    );
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .prepare_acquisition(
            &base,
            &catalog,
            &agentir_policy_eval::MeasurementAcquisitionLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    assert!(campaign.terminal_artifact_hashes.len() >= 2);
}

#[test]
fn terminal_materialization_is_canonical_and_no_eligible_is_typed() {
    let mut campaign = search_complete();
    let full = catalog(&campaign);
    let mut expected = full.artifacts.keys().cloned().collect::<Vec<_>>();
    expected.sort();
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .prepare_acquisition(
            &base,
            &full,
            &agentir_policy_eval::MeasurementAcquisitionLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    assert_eq!(campaign.terminal_artifact_hashes, expected);
    assert_eq!(campaign.work.hardware_calls, 0);

    let mut none = search_complete();
    let first = full.artifacts.values().next().unwrap().clone();
    let ineligible = MeasurementAcquisitionCatalog::synthetic_fixture(
        "ineligible".to_owned(),
        none.plan.initial_anchor_hash.clone(),
        vec![MeasurementAcquisitionArtifact {
            offline_valid: false,
            ..first
        }],
    )
    .unwrap();
    let base = none.autotuning_campaign_session_hash.clone();
    none.prepare_acquisition(
        &base,
        &ineligible,
        &agentir_policy_eval::MeasurementAcquisitionLimits::default(),
        &AutotuningCampaignLimits::default(),
    )
    .unwrap();
    assert_eq!(none.status, AutotuningCampaignStatus::NoEligibleTerminal);
    assert!(none.result.is_some());
}

#[test]
fn complete_checkpoint_resume_and_replay_never_reexecute() {
    let (mut campaign, catalog, store, executor) = complete_campaign();
    let invocations = executor.invocations;
    let result = campaign.result.clone().unwrap();
    assert_eq!(
        result.autotuning_campaign_result_hash,
        autotuning_campaign_result_hash(&result).unwrap()
    );
    let base = campaign.autotuning_campaign_session_hash.clone();
    let checkpoint = campaign
        .checkpoint(
            &base,
            &SearchLimits::default(),
            &MeasurementAcquisitionRecoveryLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    assert_eq!(
        checkpoint.autotuning_campaign_checkpoint_hash,
        "7692f5511f58ecbf7622ee4d3c2b14d8a84f070704c5e9a9ec7696fd65584df9"
    );
    assert_eq!(
        result.autotuning_campaign_result_hash,
        "7e3cb22fb125bb992a103a15aaf7be2af22d070f3d740fb65b44bebb0e99f63f"
    );
    assert_eq!(
        checkpoint.autotuning_campaign_checkpoint_hash,
        autotuning_campaign_checkpoint_hash(&checkpoint).unwrap()
    );
    let resumed = AutotuningCampaignSession::resume(
        &checkpoint,
        &store,
        Some(&catalog),
        &SearchLimits::default(),
        &MeasurementAcquisitionRecoveryLimits::default(),
        &AutotuningCampaignLimits::default(),
    )
    .unwrap();
    let replayed = resumed
        .replay(
            &store,
            Some(&catalog),
            &SearchLimits::default(),
            &MeasurementAcquisitionRecoveryLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    assert_eq!(
        replayed.autotuning_campaign_result_hash,
        result.autotuning_campaign_result_hash
    );
    assert_eq!(executor.invocations, invocations);
}

#[test]
fn lifecycle_boundaries_checkpoint_and_resume_without_hardware() {
    let empty_store = SyntheticMeasurementAcquisitionStore::default();
    let mut campaign = resume_boundary(planned_campaign(), &empty_store, None);
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .advance_search(
            &base,
            1,
            &SearchLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    campaign = resume_boundary(campaign, &empty_store, None);
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .advance_search(
            &base,
            u64::MAX,
            &SearchLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    campaign = resume_boundary(campaign, &empty_store, None);
    let catalog = catalog(&campaign);
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .prepare_acquisition(
            &base,
            &catalog,
            &agentir_policy_eval::MeasurementAcquisitionLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    campaign = resume_boundary(campaign, &store, Some(&catalog));
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .execute_prepared(
            &base,
            &mut store,
            &catalog,
            None,
            &mut executor,
            Some(MeasurementAcquisitionRecoveryFaultBoundary::AfterPublicationBeforeCheckpoint),
            &MeasurementAcquisitionRecoveryLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    campaign = resume_boundary(campaign, &store, Some(&catalog));
    let invocations = executor.invocations;
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .reconcile(
            &base,
            &store,
            &catalog,
            false,
            &MeasurementAcquisitionRecoveryLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    assert_eq!(campaign.status, AutotuningCampaignStatus::Acquiring);
    campaign = resume_boundary(campaign, &store, Some(&catalog));
    assert_eq!(executor.invocations, invocations);
    while campaign.status != AutotuningCampaignStatus::AcquisitionComplete {
        let base = campaign.autotuning_campaign_session_hash.clone();
        campaign
            .execute_prepared(
                &base,
                &mut store,
                &catalog,
                None,
                &mut executor,
                None,
                &MeasurementAcquisitionRecoveryLimits::default(),
                &AutotuningCampaignLimits::default(),
            )
            .unwrap();
    }
    let completed_invocations = executor.invocations;
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .create_cohort(&base, &store, &AutotuningCampaignLimits::default())
        .unwrap();
    campaign = resume_boundary(campaign, &store, Some(&catalog));
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .recommend(&base, &AutotuningCampaignLimits::default())
        .unwrap();
    campaign = resume_boundary(campaign, &store, Some(&catalog));
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .finalize(&base, &AutotuningCampaignLimits::default())
        .unwrap();
    let resumed = resume_boundary(campaign, &store, Some(&catalog));
    assert_eq!(resumed.status, AutotuningCampaignStatus::Complete);
    assert_eq!(executor.invocations, completed_invocations);
}

#[test]
fn crash_boundaries_require_reconciliation_and_never_silently_retry() {
    for fault in [
        MeasurementAcquisitionRecoveryFaultBoundary::BeforeBenchmark,
        MeasurementAcquisitionRecoveryFaultBoundary::AfterBenchmarkBeforePublication,
        MeasurementAcquisitionRecoveryFaultBoundary::AfterPublicationBeforeCheckpoint,
    ] {
        let mut campaign = search_complete();
        let catalog = catalog(&campaign);
        let mut base = campaign.autotuning_campaign_session_hash.clone();
        campaign
            .prepare_acquisition(
                &base,
                &catalog,
                &agentir_policy_eval::MeasurementAcquisitionLimits::default(),
                &AutotuningCampaignLimits::default(),
            )
            .unwrap();
        let mut store = SyntheticMeasurementAcquisitionStore::default();
        let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
        base.clone_from(&campaign.autotuning_campaign_session_hash);
        campaign
            .execute_prepared(
                &base,
                &mut store,
                &catalog,
                None,
                &mut executor,
                Some(fault),
                &MeasurementAcquisitionRecoveryLimits::default(),
                &AutotuningCampaignLimits::default(),
            )
            .unwrap();
        assert_eq!(campaign.status, AutotuningCampaignStatus::RecoveryRequired);
        let before = campaign.clone();
        let invocations = executor.invocations;
        let base = campaign.autotuning_campaign_session_hash.clone();
        assert!(
            campaign
                .execute_prepared(
                    &base,
                    &mut store,
                    &catalog,
                    None,
                    &mut executor,
                    None,
                    &MeasurementAcquisitionRecoveryLimits::default(),
                    &AutotuningCampaignLimits::default(),
                )
                .is_err()
        );
        assert_eq!(campaign, before);
        assert_eq!(executor.invocations, invocations);
        let base = campaign.autotuning_campaign_session_hash.clone();
        campaign
            .reconcile(
                &base,
                &store,
                &catalog,
                fault != MeasurementAcquisitionRecoveryFaultBoundary::AfterPublicationBeforeCheckpoint,
                &MeasurementAcquisitionRecoveryLimits::default(),
                &AutotuningCampaignLimits::default(),
            )
            .unwrap();
    }
}

#[test]
fn limits_are_exact_atomic_and_excluded_from_hashes() {
    let campaign = planned_campaign();
    let exact = AutotuningCampaignLimits {
        retained_sessions: 1,
        ..AutotuningCampaignLimits::default()
    };
    let recreated = AutotuningCampaignSession::start(
        campaign.search.clone(),
        campaign.ranker.clone(),
        campaign.plan.clone(),
        &exact,
    )
    .unwrap();
    assert_eq!(
        recreated.autotuning_campaign_session_hash,
        campaign.autotuning_campaign_session_hash
    );
    let rejected = AutotuningCampaignSession::start(
        campaign.search.clone(),
        campaign.ranker.clone(),
        campaign.plan.clone(),
        &AutotuningCampaignLimits {
            retained_sessions: 0,
            ..AutotuningCampaignLimits::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        rejected.code,
        EvaluationErrorCode::EvaluationAutotuningCampaignLimitExceeded
    );
}

#[test]
fn archive_v8_migration_round_trip_and_mutation_rejection() {
    let harness =
        EvaluationHarness::with_corpus_and_limits(corpus(), EvaluationLimits::default()).unwrap();
    let current = harness.archive(&[]).unwrap();
    assert_eq!(current.manifest.version, 8);
    let mut v7 = current.clone();
    v7.manifest.version = 7;
    v7.archive_hash.clear();
    v7.archive_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_V7_DOMAIN,
        &v7,
    )
    .unwrap();
    verify_archive(&v7).unwrap();
    let migrated = migrate_archive_v7_to_v8(&v7).unwrap();
    assert!(migrated.autotuning_campaign_sessions.is_empty());

    let (mut campaign, catalog, store, _) = complete_campaign();
    let base = campaign.autotuning_campaign_session_hash.clone();
    let checkpoint = campaign
        .checkpoint(
            &base,
            &SearchLimits::default(),
            &MeasurementAcquisitionRecoveryLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    campaign
        .replay(
            &store,
            Some(&catalog),
            &SearchLimits::default(),
            &MeasurementAcquisitionRecoveryLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .unwrap();
    let result = campaign.result.clone().unwrap();
    let archive = attach_autotuning_campaign_artifacts(
        &current,
        AutotuningCampaignArchiveBundle {
            sessions: vec![campaign],
            checkpoints: vec![checkpoint],
            results: vec![result.clone()],
            replay_statuses: BTreeMap::from([(
                result.autotuning_campaign_result_hash.clone(),
                true,
            )]),
        },
    )
    .unwrap();
    verify_archive(&archive).unwrap();
    let bytes = serde_json::to_vec(&archive).unwrap();
    let round_trip = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(archive, round_trip);
    let mut corrupt = archive;
    corrupt.autotuning_campaign_results[0]
        .recommended_artifact_hash
        .clone_from(&Some("corrupt".to_owned()));
    assert!(verify_archive(&corrupt).is_err());
}

#[test]
fn protocol_denies_client_claims_and_metadata() {
    let harness = EvaluationHarness::new().unwrap();
    let corpus_hash = harness.corpus().corpus_hash.clone();
    let mut protocol = agentir_policy_eval::EvaluationProtocol::new().unwrap();
    let mut request = serde_json::json!({
        "command":"evaluation.autotuning_campaign.start",
        "request_id":"campaign-start",
        "task":"candidate-rewrite-small",
        "corpus_hash":corpus_hash,
        "ranking_policy":"goal_directed_rule_v1",
        "seed":0,
        "beam_width":2,
        "maximum_semantic_depth":32,
        "maximum_children_retained_per_node":4,
        "checkpoint_cadence_work_units":1,
        "benchmark_config":{"warmups":1,"iterations":2,"input_distribution":"zero","tensor_dimensions":[4]},
        "records_per_artifact":1,
        "validation_policy":"synthetic_fixture_v1",
        "acquisition_checkpoint_cadence_slots":1,
        "measured_metric":"median_ns",
        "aggregation_method":"single_record_summary_v1",
        "indifference_band_ppm":0,
        "terminal_artifact_cap":4
    });
    let started: Value =
        serde_json::from_str(&protocol.process_line(&request.to_string())).unwrap();
    assert_eq!(started["ok"], true);
    assert_eq!(started["result"]["device_calls"], 0);
    let campaign = started["result"]["campaign"].as_str().unwrap();
    let session_hash = started["result"]["autotuning_campaign_session_hash"]
        .as_str()
        .unwrap();
    let status: Value = serde_json::from_str(
        &protocol.process_line(
            &serde_json::json!({
                "command":"evaluation.autotuning_campaign.status",
                "request_id":"campaign-status",
                "campaign":campaign,
                "autotuning_campaign_session_hash":session_hash
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(status["ok"], true);
    assert_eq!(status["result"]["device_calls"], 0);

    request["request_id"] = Value::String("campaign-forbidden".to_owned());
    request["hardware_execution_claim"] = Value::Bool(true);
    let response: Value =
        serde_json::from_str(&protocol.process_line(&request.to_string())).unwrap();
    assert_eq!(response["ok"], false);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown field")
    );
}
