use agentir_policy_eval::hashing::{ARCHIVE_HASH_V3_DOMAIN, ARCHIVE_HASH_V4_DOMAIN, domain_hash};
use agentir_policy_eval::ranking::{feature_schema_v1, scripted_ranker};
use agentir_policy_eval::{
    EvaluationHarness, EvaluationProtocol, EvaluationTaskId, LearnedModelArtifact,
    ObjectiveDirection, SearchHistoryStatus, SearchLimits, SearchObjectiveComponent,
    SearchObjectiveComponentKind, SearchObjectiveDescriptor, SearchPlan, SearchRanker,
    SearchSession, SearchStatus, attach_search_artifacts, builtin_corpus, learned_model_hash,
    learned_policy, migrate_archive_v3_to_v4, replay_search, search_checkpoint_hash,
    search_node_hash, verify_archive,
};
use serde_json::{Value, json};

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
    .unwrap()
}

fn scripted_session(beam: u64, depth: u64) -> (SearchSession, SearchRanker) {
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
    let plan = SearchPlan::deterministic_beam_v1(&objective, &ranker, beam, depth, 4, 2).unwrap();
    let session = SearchSession::start(corpus, task.id, objective, plan, &ranker).unwrap();
    (session, ranker)
}

fn zero_model() -> (LearnedModelArtifact, agentir_policy_eval::FeatureSchema) {
    let schema = feature_schema_v1().unwrap();
    let feature_names = schema
        .definitions
        .iter()
        .map(|definition| definition.name.clone())
        .chain(std::iter::once("compiler_order".to_owned()))
        .collect::<Vec<_>>();
    let mut model = LearnedModelArtifact {
        version: 1,
        model_kind: "pairwise_integer_linear_v1".to_owned(),
        ranking_dataset_hash: "stage7a-study-dataset-anchor".to_owned(),
        dataset_split_hash: "stage7a-study-split-anchor".to_owned(),
        training_configuration_hash: "stage7a-study-training-anchor".to_owned(),
        feature_schema_hash: schema.feature_schema_hash.clone(),
        feature_codec: "visible_feature_integer_codec_v1".to_owned(),
        feature_names,
        weights: vec![0; schema.definitions.len() + 1],
        bias: 0,
        score_scale: 1_000_000,
        maximum_choices: 1_000,
        learned_model_hash: String::new(),
    };
    model.learned_model_hash = learned_model_hash(&model).unwrap();
    (model, schema)
}

#[test]
fn beam_widths_horizon_and_replay_are_deterministic() {
    for beam in [1, 2, 4] {
        let (mut session, ranker) = scripted_session(beam, 32);
        session
            .advance(u64::MAX, &ranker, &SearchLimits::default())
            .unwrap();
        assert!(matches!(
            session.status,
            SearchStatus::Complete | SearchStatus::Bounded
        ));
        assert!(session.result.is_some());
        assert!(
            session
                .result
                .as_ref()
                .unwrap()
                .recommendation_semantics
                .contains("observed terminal")
        );
        replay_search(&session, &ranker, &SearchLimits::default()).unwrap();
    }
    let (bounded, _) = scripted_session(2, 0);
    assert_eq!(bounded.status, SearchStatus::Bounded);
    assert_eq!(bounded.nodes.len(), 1);
    assert!(bounded.edges.is_empty());
}

#[test]
fn learned_search_uses_unchanged_visible_frame_and_corruption_is_atomic() {
    let corpus = builtin_corpus().unwrap();
    let task = corpus
        .tasks
        .iter()
        .find(|task| task.id.0 == "candidate-rewrite-small")
        .unwrap()
        .clone();
    let (model, schema) = zero_model();
    let descriptor = learned_policy(
        &model,
        agentir_policy_eval::PolicyKind::Menu,
        false,
        1_000_000_000,
    )
    .unwrap();
    let ranker = SearchRanker::Learned {
        descriptor,
        model: Box::new(model),
        schema,
    };
    let objective = objective(&corpus, &task);
    let plan = SearchPlan::deterministic_beam_v1(&objective, &ranker, 2, 32, 2, 1).unwrap();
    let mut session = SearchSession::start(
        corpus.clone(),
        task.id.clone(),
        objective.clone(),
        plan,
        &ranker,
    )
    .unwrap();
    session
        .advance(1, &ranker, &SearchLimits::default())
        .unwrap();
    assert_eq!(session.work.ranking_inferences, 1);
    assert!(session.edges.values().all(|edge| edge.inference.is_some()));

    let SearchRanker::Learned {
        descriptor,
        mut model,
        schema,
    } = ranker
    else {
        unreachable!()
    };
    model.weights[0] = 1;
    let corrupt = SearchRanker::Learned {
        descriptor,
        model,
        schema,
    };
    let diagnostic = SearchPlan::deterministic_beam_v1(&objective, &corrupt, 1, 32, 1, 1)
        .expect_err("corrupt retained model changes its anchored identity");
    assert_eq!(
        diagnostic.code,
        agentir_policy_eval::EvaluationErrorCode::EvaluationModelIncompatible
    );
}

#[test]
fn archive_v4_migration_does_not_invent_search_and_search_round_trips() {
    let mut harness = EvaluationHarness::new().unwrap();
    let run = harness
        .run_scripted(
            "free_reference_v1",
            &[EvaluationTaskId("saxpy-end-to-end-large".to_owned())],
            &[0],
        )
        .unwrap();
    let mut legacy = harness.archive(&[run]).unwrap();
    legacy.manifest.version = 3;
    legacy.search_history_status = SearchHistoryStatus::Unspecified;
    legacy.measured_search_history_status =
        agentir_policy_eval::MeasuredSearchHistoryStatus::Unspecified;
    legacy.archive_hash.clear();
    legacy.archive_hash = domain_hash(ARCHIVE_HASH_V3_DOMAIN, &legacy).unwrap();
    verify_archive(&legacy).unwrap();
    let migrated = migrate_archive_v3_to_v4(&legacy).unwrap();
    assert_eq!(
        migrated.search_history_status,
        SearchHistoryStatus::NoSearchHistory
    );
    assert!(migrated.search_runs.is_empty());

    let (mut session, ranker) = scripted_session(2, 32);
    session
        .advance(u64::MAX, &ranker, &SearchLimits::default())
        .unwrap();
    let checkpoint = session.checkpoint(&SearchLimits::default()).unwrap();
    let archive = attach_search_artifacts(&migrated, &[(session.clone(), checkpoint)]).unwrap();
    assert_eq!(
        archive.search_history_status,
        SearchHistoryStatus::SearchHistoryPresent
    );
    assert_eq!(archive.search_runs.len(), 1);
    verify_archive(&archive).unwrap();

    let mut checksum_mutation = archive.clone();
    checksum_mutation.archive_hash.push('0');
    assert!(verify_archive(&checksum_mutation).is_err());

    let mut node_mutation = archive;
    node_mutation.search_checkpoints[0]
        .session
        .nodes
        .get_mut(&agentir_policy_eval::SearchNodeId(
            "search-node-1".to_owned(),
        ))
        .unwrap()
        .depth = 1;
    node_mutation.search_checkpoints[0]
        .search_checkpoint_hash
        .clear();
    node_mutation.search_checkpoints[0].search_checkpoint_hash =
        search_checkpoint_hash(&node_mutation.search_checkpoints[0]).unwrap();
    node_mutation.search_runs[0].checkpoint_hashes = vec![
        node_mutation.search_checkpoints[0]
            .search_checkpoint_hash
            .clone(),
    ];
    node_mutation.archive_hash.clear();
    node_mutation.archive_hash = domain_hash(ARCHIVE_HASH_V4_DOMAIN, &node_mutation).unwrap();
    assert!(verify_archive(&node_mutation).is_err());
}

#[test]
fn failed_checkpoint_and_stale_root_are_atomic() {
    let (session, ranker) = scripted_session(1, 32);
    let before = session.clone();
    let limits = SearchLimits {
        checkpoint_bytes: 1,
        ..SearchLimits::default()
    };
    assert!(session.checkpoint(&limits).is_err());
    assert_eq!(session, before);

    let mut stale = session.objective.clone();
    stale.initial_anchor_hash = "stale".to_owned();
    assert!(
        SearchSession::start(
            session.corpus.clone(),
            session.task_id.clone(),
            stale,
            session.plan.clone(),
            &ranker,
        )
        .is_err()
    );
    assert!(session.edges.is_empty());

    let (mut exhausted, ranker) = scripted_session(1, 32);
    let before_nodes = exhausted.nodes.clone();
    let limits = SearchLimits {
        semantic_expansions: 0,
        ..SearchLimits::default()
    };
    exhausted.advance(1, &ranker, &limits).unwrap();
    assert_eq!(exhausted.status, SearchStatus::Failed);
    assert_eq!(exhausted.nodes, before_nodes);
    assert!(exhausted.edges.is_empty());
}

#[test]
fn jsonl_search_commands_are_bounded_and_hash_anchored() {
    let mut protocol = EvaluationProtocol::new().unwrap();
    let corpus_hash = builtin_corpus().unwrap().corpus_hash;
    let start_line = json!({
        "command": "evaluation.search.start",
        "request_id": "s1",
        "task": "candidate-rewrite-small",
        "corpus_hash": corpus_hash,
        "ranking_policy": "goal_directed_rule_v1",
        "seed": 0,
        "beam_width": 1,
        "maximum_semantic_depth": 32,
        "maximum_children_retained_per_node": 2,
        "checkpoint_cadence_work_units": 2
    })
    .to_string();
    let start: Value = serde_json::from_str(&protocol.process_line(&start_line)).unwrap();
    assert_eq!(start["ok"], true);
    let result = &start["result"];
    let search = result["search"].as_str().unwrap();
    let objective_hash = result["search_objective_hash"].as_str().unwrap();
    let plan_hash = result["search_plan_hash"].as_str().unwrap();
    let stale: Value = serde_json::from_str(
        &protocol.process_line(
            &json!({
                "command": "evaluation.search.advance",
                "request_id": "s2",
                "search": search,
                "search_objective_hash": "stale",
                "search_plan_hash": plan_hash,
                "maximum_work_units": 1
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(stale["ok"], false);
    let advance: Value = serde_json::from_str(
        &protocol.process_line(
            &json!({
                "command": "evaluation.search.advance",
                "request_id": "s3",
                "search": search,
                "search_objective_hash": objective_hash,
                "search_plan_hash": plan_hash,
                "maximum_work_units": 1000
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(advance["ok"], true);
    let replay: Value = serde_json::from_str(
        &protocol.process_line(
            &json!({
                "command": "evaluation.search.replay",
                "request_id": "s4",
                "search": search,
                "search_objective_hash": objective_hash,
                "search_plan_hash": plan_hash
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(replay["ok"], true);
    assert_eq!(replay["result"]["network_calls"], 0);
}

#[test]
fn mutation_helpers_preserve_domain_separation() {
    let (session, _) = scripted_session(1, 0);
    let root = session.nodes.values().next().unwrap();
    let mut changed = root.clone();
    changed.task_success = !changed.task_success;
    assert_ne!(root.search_node_hash, search_node_hash(&changed).unwrap());
    assert_ne!(
        session.objective.search_objective_hash,
        session.plan.search_plan_hash
    );
}
