use agentir_policy_eval::{
    ChoiceCategory, ChoiceOrigin, ChoicePreconditions, EvaluationChoiceId, EvaluationErrorCode,
    EvaluationHarness, EvaluationTaskId, FixedScore, RankingDecision, RankingEpisodeStatus,
    RankingLimits, RankingSubmission, SelectionSource, build_choice_set, builtin_ranked_corpus,
    compiler_choice, feature_schema_hash, feature_schema_v1, migrate_archive_v1_to_v2,
    rank_choices, ranked_policy, replay_ranking_trace, scripted_ranker, scripted_ranking_decision,
    verify_archive,
};
use agentir_policy_eval::{PolicyOrigin, hashing::domain_hash};
use serde_json::json;

fn choice(command: &str, request_id: &str, target: &str) -> agentir_policy_eval::EvaluationChoice {
    compiler_choice(
        ChoiceOrigin::Schedule,
        ChoiceCategory::ScheduleTile,
        json!({"command":command,"request_id":request_id,"target":target}),
        ChoicePreconditions::default(),
        format!("compiler choice for {target}"),
        "unchanged_or_compiler_owned",
        target,
    )
    .unwrap()
}

#[test]
fn choice_identity_order_hash_and_schema_are_deterministic() {
    let schema = feature_schema_v1().unwrap();
    assert_eq!(
        schema.feature_schema_hash,
        feature_schema_hash(&schema).unwrap()
    );
    let limits = RankingLimits::default();
    let first = build_choice_set(
        "observation",
        &schema,
        vec![
            choice("schedule.apply", "a", "sa1"),
            choice("schedule.apply", "b", "sa2"),
        ],
        &limits,
    )
    .unwrap();
    let repeated = build_choice_set(
        "observation",
        &schema,
        vec![
            choice("schedule.apply", "a", "sa1"),
            choice("schedule.apply", "b", "sa2"),
        ],
        &limits,
    )
    .unwrap();
    assert_eq!(first, repeated);
    let reordered = build_choice_set(
        "observation",
        &schema,
        vec![
            choice("schedule.apply", "b", "sa2"),
            choice("schedule.apply", "a", "sa1"),
        ],
        &limits,
    )
    .unwrap();
    assert_ne!(first.choice_set_hash, reordered.choice_set_hash);
    assert_eq!(first.choices[0].id, reordered.choices[1].id);
    assert_eq!(first.choices[1].id, reordered.choices[0].id);

    let transport_mutated = build_choice_set(
        "observation",
        &schema,
        vec![
            choice("schedule.apply", "different-request", "sa1"),
            choice("schedule.apply", "different-request-2", "sa2"),
        ],
        &limits,
    )
    .unwrap();
    assert_eq!(first.choices[0].id, transport_mutated.choices[0].id);
    assert_eq!(first.choices[1].id, transport_mutated.choices[1].id);

    let mut reordered_schema = schema.clone();
    reordered_schema.definitions.swap(0, 1);
    reordered_schema.feature_schema_hash.clear();
    assert_ne!(
        first.feature_schema_hash,
        feature_schema_hash(&reordered_schema).unwrap()
    );
}

#[test]
fn transport_metadata_does_not_change_semantic_choice_identity() {
    let schema = feature_schema_v1().unwrap();
    let limits = RankingLimits::default();
    let first = compiler_choice(
        ChoiceOrigin::Schedule,
        ChoiceCategory::ScheduleTile,
        json!({
            "command":"schedule.apply",
            "request_id":"first",
            "correlation_id":"provider-a",
            "timestamp_ns":1,
            "latency_ms":10,
            "hostname":"host-a",
            "provider_session_id":"secret-a",
            "actions":[{"kind":"tile_axes","axes":["sa1"],"tile_sizes":[4]}]
        }),
        ChoicePreconditions::default(),
        "tile",
        "advance",
        "sa1",
    )
    .unwrap();
    let second = compiler_choice(
        ChoiceOrigin::Schedule,
        ChoiceCategory::ScheduleTile,
        json!({
            "command":"schedule.apply",
            "request_id":"second",
            "correlation_id":"provider-b",
            "timestamp_ns":2,
            "latency_ms":20,
            "hostname":"host-b",
            "provider_session_id":"secret-b",
            "actions":[{"kind":"tile_axes","axes":["sa1"],"tile_sizes":[4]}]
        }),
        ChoicePreconditions::default(),
        "tile",
        "advance",
        "sa1",
    )
    .unwrap();
    let first = build_choice_set("observation", &schema, vec![first], &limits).unwrap();
    let second = build_choice_set("observation", &schema, vec![second], &limits).unwrap();
    assert_eq!(first.choices[0].id, second.choices[0].id);

    let production_mutated = build_choice_set(
        "observation",
        &schema,
        vec![choice("schedule.apply", "third", "sa2")],
        &limits,
    )
    .unwrap();
    assert_ne!(first.choices[0].id, production_mutated.choices[0].id);
}

#[test]
fn visible_feature_vectors_are_exact_typed_and_normalized() {
    let schema = feature_schema_v1().unwrap();
    let limits = RankingLimits::default();
    let valid = choice("schedule.apply", "feature-valid", "sa1");

    let mut missing = valid.clone();
    missing.visible_features.values.remove("structural_target");
    assert_eq!(
        build_choice_set("observation", &schema, vec![missing], &limits)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationFeatureSchemaMismatch
    );

    let mut wrong_type = valid.clone();
    wrong_type.visible_features.values.insert(
        "operand_arity".to_owned(),
        agentir_policy_eval::FeatureValue::Text("zero".to_owned()),
    );
    assert_eq!(
        build_choice_set("observation", &schema, vec![wrong_type], &limits)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationFeatureSchemaMismatch
    );

    let mut negative = valid.clone();
    negative.visible_features.values.insert(
        "static_extent_count".to_owned(),
        agentir_policy_eval::FeatureValue::Integer(-1),
    );
    assert_eq!(
        build_choice_set("observation", &schema, vec![negative], &limits)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationFeatureSchemaMismatch
    );

    let mut capabilities = valid;
    capabilities.visible_features.values.insert(
        "target_capabilities".to_owned(),
        agentir_policy_eval::FeatureValue::TextList(vec![
            "z".to_owned(),
            "a".to_owned(),
            "a".to_owned(),
        ]),
    );
    assert_eq!(
        build_choice_set("observation", &schema, vec![capabilities], &limits)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationFeatureSchemaMismatch
    );
}

#[test]
fn archive_metric_floats_round_trip_without_ulp_drift() {
    let metric = 140_f64 / 142_f64;
    let bytes = serde_json::to_vec(&metric).unwrap();
    let decoded: f64 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(metric.to_bits(), decoded.to_bits());
}

#[test]
fn fixed_scores_and_ties_are_checked_and_replayable() {
    let schema = feature_schema_v1().unwrap();
    let limits = RankingLimits::default();
    let set = build_choice_set(
        "observation",
        &schema,
        vec![
            choice("schedule.apply", "a", "sa1"),
            choice("schedule.apply", "b", "sa2"),
        ],
        &limits,
    )
    .unwrap();
    let policy = scripted_ranker("goal_directed_rule_v1", &schema, 7).unwrap();
    let tied = RankingDecision::Scores {
        scores: set
            .choices
            .iter()
            .map(|choice| agentir_policy_eval::ChoiceScore {
                choice_id: choice.id.clone(),
                score: FixedScore::v1(10, limits.score_magnitude).unwrap(),
            })
            .collect(),
    };
    let trace = rank_choices(&set, &policy, tied, &limits).unwrap();
    assert_eq!(trace.selected_choice.as_ref(), Some(&set.choices[0].id));
    assert_eq!(trace.selection_source, SelectionSource::RankedTopOne);
    replay_ranking_trace(&set, &policy, &trace, &limits).unwrap();

    let missing = RankingDecision::Scores { scores: vec![] };
    assert_eq!(
        rank_choices(&set, &policy, missing, &limits)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationScoreCountMismatch
    );
    assert_eq!(
        FixedScore::v1(i64::MAX, limits.score_magnitude)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationScoreInvalid
    );
}

#[test]
fn scripted_rankers_are_reproducible_and_do_not_change_choice_sets() {
    let schema = feature_schema_v1().unwrap();
    let limits = RankingLimits::default();
    let set = build_choice_set(
        "observation",
        &schema,
        vec![
            choice("schedule.apply", "a", "sa1"),
            choice("schedule.apply", "b", "sa2"),
        ],
        &limits,
    )
    .unwrap();
    for name in [
        "lexicographic_choice_v1",
        "first_progress_choice_v1",
        "goal_directed_rule_v1",
        "proof_frontier_first_v1",
        "min_context_choice_v1",
        "seeded_uniform_choice_v1",
        "hybrid_ranked_escape_v1",
    ] {
        let policy = scripted_ranker(name, &schema, 11).unwrap();
        assert_eq!(
            scripted_ranking_decision(&policy, &set, &limits).unwrap(),
            scripted_ranking_decision(&policy, &set, &limits).unwrap()
        );
    }
    let first = scripted_ranker("seeded_uniform_choice_v1", &schema, 1).unwrap();
    let second = scripted_ranker("seeded_uniform_choice_v1", &schema, 2).unwrap();
    assert_ne!(first.ranking_policy_hash, second.ranking_policy_hash);
    assert_eq!(set.choice_set_hash, set.choice_set_hash);
}

#[test]
fn real_memory_continuation_produces_bounded_multi_choice_frame() {
    let schema = feature_schema_v1().unwrap();
    let ranker = scripted_ranker("goal_directed_rule_v1", &schema, 0).unwrap();
    let mut harness = EvaluationHarness::new().unwrap();
    let policy = ranked_policy(&ranker, PolicyOrigin::External).unwrap();
    let run = harness
        .start_run(
            policy,
            &[EvaluationTaskId("memory-reuse-medium".to_owned())],
            &[0],
        )
        .unwrap();
    let episode = harness.run(&run).unwrap().episodes[0].id.clone();
    loop {
        let observation = harness.next_observation(&episode).unwrap();
        if observation
            .previous_compiler_outcome
            .as_ref()
            .and_then(|value| value.pointer("/result/reuse_choices"))
            .is_some()
        {
            let set = harness.ranked_choice_set(&episode).unwrap();
            assert!(set.choices.len() >= 3);
            assert!(
                set.choices
                    .iter()
                    .any(|choice| choice.category == ChoiceCategory::MemoryFresh)
            );
            assert!(
                set.choices
                    .iter()
                    .any(|choice| choice.category == ChoiceCategory::MemoryStaticReuse)
            );
            break;
        }
        let set = harness.ranked_choice_set(&episode).unwrap();
        harness
            .rank_episode(RankingSubmission {
                episode_id: episode.clone(),
                step_id: observation.step_id.clone(),
                observation_hash: observation.observation_hash.clone(),
                choice_set_hash: observation.choice_set_hash.clone().unwrap(),
                feature_schema_hash: observation.feature_schema_hash.clone().unwrap(),
                ranking_policy_hash: ranker.ranking_policy_hash.clone(),
                decision: RankingDecision::Select {
                    choice_id: set.choices[0].id.clone(),
                },
                usage: None,
                correlation_id: None,
            })
            .unwrap();
    }
}

#[test]
fn archive_v1_migration_is_explicitly_unranked() {
    let mut harness = EvaluationHarness::new().unwrap();
    let run = harness
        .run_scripted(
            "free_reference_v1",
            &[EvaluationTaskId("saxpy-end-to-end-large".to_owned())],
            &[0],
        )
        .unwrap();
    let mut legacy = harness.archive(&[run]).unwrap();
    legacy.manifest.version = 1;
    legacy.feature_schemas.clear();
    legacy.ranking_policies.clear();
    legacy.choice_sets.clear();
    legacy.ranking_statuses.clear();
    legacy.ranking_datasets.clear();
    legacy.dataset_splits.clear();
    legacy.training_configurations.clear();
    legacy.training_runs.clear();
    legacy.learned_models.clear();
    legacy.ranking_inputs.clear();
    legacy.inference_records.clear();
    legacy.learning_statuses.clear();
    legacy.archive_hash.clear();
    legacy.archive_hash =
        domain_hash(agentir_policy_eval::hashing::ARCHIVE_HASH_DOMAIN, &legacy).unwrap();
    verify_archive(&legacy).unwrap();
    let migrated = migrate_archive_v1_to_v2(&legacy).unwrap();
    assert_eq!(migrated.manifest.version, 2);
    assert!(
        migrated
            .ranking_statuses
            .values()
            .all(|status| *status == RankingEpisodeStatus::Unranked)
    );
    verify_archive(&migrated).unwrap();
}

#[test]
fn explicit_selection_must_belong_to_exact_frame() {
    let schema = feature_schema_v1().unwrap();
    let limits = RankingLimits::default();
    let set = build_choice_set(
        "observation",
        &schema,
        vec![choice("schedule.apply", "a", "sa1")],
        &limits,
    )
    .unwrap();
    let policy = scripted_ranker("lexicographic_choice_v1", &schema, 0).unwrap();
    let error = rank_choices(
        &set,
        &policy,
        RankingDecision::Select {
            choice_id: EvaluationChoiceId("ec-not-visible".to_owned()),
        },
        &limits,
    )
    .unwrap_err();
    assert_eq!(
        error.code,
        EvaluationErrorCode::EvaluationSelectionNotInChoiceSet
    );
}

#[test]
fn ranked_corpus_is_versioned_ordered_and_has_small_medium_large_tasks() {
    let first = builtin_ranked_corpus().unwrap();
    let second = builtin_ranked_corpus().unwrap();
    assert_eq!(first, second);
    assert_eq!(first.tasks.len(), 14);
    for size in ["small", "medium", "large"] {
        assert!(first.tasks.iter().any(|task| task.size == size));
    }
}

#[test]
fn hybrid_escape_is_explicit_and_menu_policy_rejects_it() {
    let schema = feature_schema_v1().unwrap();
    let limits = RankingLimits::default();
    let set = build_choice_set(
        "observation",
        &schema,
        vec![choice("workspace.open", "menu", "frame")],
        &limits,
    )
    .unwrap();
    let action = json!({"command":"workspace.open","request_id":"escape","workspace":"w1"});
    let hybrid = scripted_ranker("hybrid_ranked_escape_v1", &schema, 0).unwrap();
    let trace = rank_choices(
        &set,
        &hybrid,
        RankingDecision::HybridEscape {
            action: action.clone(),
        },
        &limits,
    )
    .unwrap();
    assert_eq!(trace.selection_source, SelectionSource::HybridEscape);
    assert!(trace.selected_choice.is_none());
    let menu = scripted_ranker("lexicographic_choice_v1", &schema, 0).unwrap();
    assert_eq!(
        rank_choices(
            &set,
            &menu,
            RankingDecision::HybridEscape { action },
            &limits,
        )
        .unwrap_err()
        .code,
        EvaluationErrorCode::EvaluationRankingPolicyInvalid
    );
}
