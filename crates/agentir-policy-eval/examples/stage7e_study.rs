use agentir_core::{backend_ir::HardwareBenchmarkConfig, ids::ArtifactId};
use agentir_policy_eval::ranking::{feature_schema_v1, scripted_ranker};
use agentir_policy_eval::{
    AutotuningCampaignArchiveBundle, AutotuningCampaignLimits, AutotuningCampaignPlan,
    AutotuningCampaignSession, AutotuningCampaignStatus, EvaluationCorpus, EvaluationHarness,
    EvaluationLimits, MeasuredMetric, MeasurementAcquisitionArtifact,
    MeasurementAcquisitionCatalog, MeasurementAcquisitionLimits,
    MeasurementAcquisitionRecoveryFaultBoundary, MeasurementAcquisitionRecoveryLimits,
    MeasurementAggregationMethod, MeasurementValidationPolicy, ObjectiveDirection, SearchLimits,
    SearchObjectiveComponent, SearchObjectiveComponentKind, SearchObjectiveDescriptor, SearchPlan,
    SearchRanker, SearchSession, SyntheticMeasurementAcquisitionExecutor,
    SyntheticMeasurementAcquisitionStore, attach_autotuning_campaign_artifacts,
    builtin_ranked_corpus, hashing, terminal_artifact_hash, verify_archive,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    time::Instant,
};

fn output_path() -> Result<PathBuf, String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() == 2 && args[0] == "--output" {
        Ok(PathBuf::from(&args[1]))
    } else {
        Err("usage: stage7e_study --output PATH".to_owned())
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn study_corpus() -> EvaluationCorpus {
    let mut corpus = builtin_ranked_corpus().expect("ranked corpus");
    let requests = include_str!("../../../examples/stage7e_two_artifact.jsonl")
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
    task.budget.max_actions = u64::try_from(task.initial_state.production_requests.len())
        .expect("request count")
        .saturating_add(4);
    task.budget.max_rejections = 4;
    "materialize two production-replayed terminal artifacts"
        .clone_into(&mut task.objective.summary);
    task.metadata.insert(
        "stage7e_fixture".to_owned(),
        "synthetic_test_data_not_performance_evidence".to_owned(),
    );
    "stage7e-integrated-campaign-study".clone_into(&mut corpus.name);
    "stage7e-study-v1".clone_into(&mut corpus.version);
    corpus.corpus_hash.clear();
    corpus.corpus_hash =
        hashing::domain_hash(hashing::CORPUS_HASH_DOMAIN, &corpus).expect("corpus hash");
    corpus
}

fn planned(corpus: EvaluationCorpus) -> AutotuningCampaignSession {
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
    let ranker = SearchRanker::Scripted {
        descriptor: scripted_ranker(
            "goal_directed_rule_v1",
            &feature_schema_v1().expect("schema"),
            0,
        )
        .expect("ranker"),
    };
    let search_plan =
        SearchPlan::deterministic_beam_v1(&objective, &ranker, 4, 64, 4, 1).expect("search plan");
    let search = SearchSession::start(corpus, task.id, objective, search_plan, &ranker)
        .expect("search start");
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
    .expect("campaign plan");
    AutotuningCampaignSession::start(search, ranker, plan, &AutotuningCampaignLimits::default())
        .expect("campaign start")
}

fn advance_search(campaign: &mut AutotuningCampaignSession) {
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .advance_search(
            &base,
            u64::MAX,
            &SearchLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .expect("search completion");
}

fn catalog(campaign: &AutotuningCampaignSession) -> MeasurementAcquisitionCatalog {
    let artifacts = campaign
        .search
        .nodes
        .values()
        .filter(|node| node.terminal)
        .filter_map(|node| production_terminal_artifact(campaign, &node.id))
        .collect::<Vec<_>>();
    assert!(artifacts.len() >= 2, "two terminal artifacts");
    MeasurementAcquisitionCatalog::synthetic_fixture(
        "stage7e-production-replayed-terminal-paths".to_owned(),
        campaign.plan.initial_anchor_hash.clone(),
        artifacts,
    )
    .expect("catalog")
}

fn production_terminal_artifact(
    campaign: &AutotuningCampaignSession,
    terminal: &agentir_policy_eval::SearchNodeId,
) -> Option<MeasurementAcquisitionArtifact> {
    let artifact_hash =
        terminal_artifact_hash(&campaign.search, terminal).expect("terminal artifact")?;
    let mut node = campaign.search.nodes.get(terminal).expect("terminal node");
    let mut spec_hashes = BTreeSet::new();
    let mut target_hashes = BTreeSet::new();
    let mut publication = None;
    while let Some(edge_id) = &node.parent_edge {
        let edge = campaign.search.edges.get(edge_id).expect("terminal edge");
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
            let result = result.expect("publication result");
            assert!(edge.compiler_outcome.accepted, "publication accepted");
            assert_eq!(
                result.get("equivalent_to_backend"),
                Some(&Value::Bool(true))
            );
            assert_eq!(result.get("offline_valid"), Some(&Value::Bool(true)));
            let query = result.get("query").expect("artifact query");
            let status = match query.get("status").and_then(Value::as_str) {
                Some("validated") => agentir_core::backend_ir::ArtifactStatus::Validated,
                Some("sealed") => agentir_core::backend_ir::ArtifactStatus::Sealed,
                other => panic!("unexpected terminal artifact status: {other:?}"),
            };
            publication = Some((
                ArtifactId::new(
                    query
                        .get("artifact")
                        .and_then(Value::as_str)
                        .expect("artifact id"),
                ),
                query
                    .get("compiler_build_hash")
                    .and_then(Value::as_str)
                    .expect("compiler build hash")
                    .to_owned(),
                status,
            ));
        }
        node = campaign
            .search
            .nodes
            .get(&edge.parent)
            .expect("parent node");
    }
    let (artifact_id, compiler_build_hash, status) = publication.expect("publication");
    assert_eq!(spec_hashes.len(), 1, "one spec hash");
    assert_eq!(target_hashes.len(), 1, "one target hash");
    Some(MeasurementAcquisitionArtifact {
        artifact_id,
        artifact_hash,
        spec_hash: spec_hashes.into_iter().next().expect("spec hash"),
        target_hash: target_hashes.into_iter().next().expect("target hash"),
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

fn prepare(campaign: &mut AutotuningCampaignSession, catalog: &MeasurementAcquisitionCatalog) {
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .prepare_acquisition(
            &base,
            catalog,
            &MeasurementAcquisitionLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .expect("prepare acquisition");
}

fn execute(
    campaign: &mut AutotuningCampaignSession,
    catalog: &MeasurementAcquisitionCatalog,
    store: &mut SyntheticMeasurementAcquisitionStore,
    executor: &mut SyntheticMeasurementAcquisitionExecutor,
    fault: Option<MeasurementAcquisitionRecoveryFaultBoundary>,
) {
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .execute_prepared(
            &base,
            store,
            catalog,
            None,
            executor,
            fault,
            &MeasurementAcquisitionRecoveryLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .expect("explicit execution");
}

fn main() -> Result<(), String> {
    let output = output_path()?;
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let corpus = study_corpus();
    let mut campaign = planned(corpus.clone());
    advance_search(&mut campaign);
    let catalog = catalog(&campaign);
    prepare(&mut campaign, &catalog);
    let mut store = SyntheticMeasurementAcquisitionStore::default();
    let mut executor = SyntheticMeasurementAcquisitionExecutor::new();
    while campaign.status != AutotuningCampaignStatus::AcquisitionComplete {
        execute(&mut campaign, &catalog, &mut store, &mut executor, None);
    }
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .create_cohort(&base, &store, &AutotuningCampaignLimits::default())
        .map_err(|error| error.to_string())?;
    let base = campaign.autotuning_campaign_session_hash.clone();
    campaign
        .recommend(&base, &AutotuningCampaignLimits::default())
        .map_err(|error| error.to_string())?;
    let base = campaign.autotuning_campaign_session_hash.clone();
    let result = campaign
        .finalize(&base, &AutotuningCampaignLimits::default())
        .map_err(|error| error.to_string())?;
    let base = campaign.autotuning_campaign_session_hash.clone();
    let checkpoint = campaign
        .checkpoint(
            &base,
            &SearchLimits::default(),
            &MeasurementAcquisitionRecoveryLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .map_err(|error| error.to_string())?;
    let resumed = AutotuningCampaignSession::resume(
        &checkpoint,
        &store,
        Some(&catalog),
        &SearchLimits::default(),
        &MeasurementAcquisitionRecoveryLimits::default(),
        &AutotuningCampaignLimits::default(),
    )
    .map_err(|error| error.to_string())?;
    let replay = resumed
        .replay(
            &store,
            Some(&catalog),
            &SearchLimits::default(),
            &MeasurementAcquisitionRecoveryLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .map_err(|error| error.to_string())?;

    let mut scenarios = Vec::new();
    for fault in [
        MeasurementAcquisitionRecoveryFaultBoundary::BeforeBenchmark,
        MeasurementAcquisitionRecoveryFaultBoundary::AfterBenchmarkBeforePublication,
        MeasurementAcquisitionRecoveryFaultBoundary::AfterPublicationBeforeCheckpoint,
    ] {
        let mut scenario = planned(corpus.clone());
        advance_search(&mut scenario);
        prepare(&mut scenario, &catalog);
        let mut scenario_store = SyntheticMeasurementAcquisitionStore::default();
        let mut scenario_executor = SyntheticMeasurementAcquisitionExecutor::new();
        execute(
            &mut scenario,
            &catalog,
            &mut scenario_store,
            &mut scenario_executor,
            Some(fault),
        );
        let base = scenario.autotuning_campaign_session_hash.clone();
        let status = scenario
            .reconcile(
                &base,
                &scenario_store,
                &catalog,
                fault != MeasurementAcquisitionRecoveryFaultBoundary::AfterPublicationBeforeCheckpoint,
                &MeasurementAcquisitionRecoveryLimits::default(),
                &AutotuningCampaignLimits::default(),
            )
            .map_err(|error| error.to_string())?;
        scenarios.push(json!({
            "fault":fault,
            "status_after_reconciliation":status,
            "executor_invocations":scenario_executor.invocations,
            "automatic_retry":false,
        }));
    }
    let mut cancelled = planned(corpus.clone());
    let base = cancelled.autotuning_campaign_session_hash.clone();
    cancelled
        .cancel(
            &base,
            &SearchLimits::default(),
            &AutotuningCampaignLimits::default(),
        )
        .map_err(|error| error.to_string())?;
    scenarios.push(json!({"cancellation_status":cancelled.status,"device_calls":0}));

    let harness = EvaluationHarness::with_corpus_and_limits(corpus, EvaluationLimits::default())
        .map_err(|error| error.to_string())?;
    let archive = attach_autotuning_campaign_artifacts(
        &harness.archive(&[]).map_err(|error| error.to_string())?,
        AutotuningCampaignArchiveBundle {
            sessions: vec![campaign.clone()],
            checkpoints: vec![checkpoint.clone()],
            results: vec![result.clone()],
            replay_statuses: BTreeMap::from([(
                result.autotuning_campaign_result_hash.clone(),
                true,
            )]),
        },
    )
    .map_err(|error| error.to_string())?;
    verify_archive(&archive).map_err(|error| error.to_string())?;
    let mut corrupt = archive.clone();
    "mutated".clone_into(&mut corrupt.autotuning_campaign_results[0].provenance);

    write_json(&output.join("campaign-plan.json"), &campaign.plan)?;
    write_json(&output.join("campaign-session.json"), &campaign)?;
    write_json(&output.join("campaign-checkpoint.json"), &checkpoint)?;
    write_json(&output.join("campaign-result.json"), &result)?;
    write_json(&output.join("scenarios.json"), &scenarios)?;
    write_json(
        &output.join("replay.json"),
        &json!({"result":replay,"device_calls":0,"benchmark_calls":0}),
    )?;
    write_json(
        &output.join("mutations.json"),
        &json!({"campaign_result_mutation_rejected":verify_archive(&corrupt).is_err()}),
    )?;
    write_json(&output.join("archive-v8.json"), &archive)?;
    write_json(
        &output.join("metrics.json"),
        &json!({
            "schema_version":"agentir.stage7e.study.v2",
            "fixture_label":"synthetic_test_data_not_performance_evidence",
            "terminal_nodes_inspected":campaign.work.terminal_nodes_inspected,
            "distinct_terminal_artifacts":catalog.artifacts.len(),
            "retained_artifact_hashes_count":campaign.terminal_artifact_hashes.len(),
            "acquisition_slots":campaign.acquisition_session.as_ref().map_or(0, |value| value.slots.len()),
            "recovery_scenarios":3,
            "replay_device_calls":0,
            "archive_version":archive.manifest.version,
            "semantic_result_hash":result.autotuning_campaign_result_hash,
        }),
    )?;
    write_json(
        &output.join("operational.json"),
        &json!({"wall_clock_ms":started.elapsed().as_millis()}),
    )?;
    Ok(())
}
