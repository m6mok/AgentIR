use agentir_policy_eval::{
    ChoiceCategory, ChoiceOrigin, ChoicePreconditions, EvaluationErrorCode, FixedScore,
    LearnedArchiveBundle, LearningEpisodeStatus, PolicyKind, PolicyOrigin, RepairAnchor,
    RepairCode, TrainingConfiguration, WorkUnitCounters, attach_learning_artifacts,
    build_choice_set, build_dataset, compiler_choice,
    continuation::{
        ContinuationAnchors, ContinuationCursor, ContinuationStatus, FrameCompleteness,
        paginate_choice_set,
    },
    dataset_split_hash, feature_schema_v1, infer, inference_hash, learned_policy,
    migrate_archive_v2_to_v3, ranked_policy, ranking_input, ranking_input_hash,
    repairs::{typed_repair, validate_repair},
    split_dataset, train_linear_ranker, training_configuration_hash, validate_dataset,
    verify_archive, verify_inference,
};
use agentir_policy_eval::{
    EvaluationHarness, EvaluationTaskId, LearnedRankingLimits, RankingLabel, RankingLimits,
    SplitKind,
};
use serde_json::json;
use std::collections::BTreeMap;

fn set(name: &str, choices: usize) -> agentir_policy_eval::EvaluationChoiceSet {
    let schema = feature_schema_v1().unwrap();
    let values = (0..choices)
        .map(|index| {
            compiler_choice(
                ChoiceOrigin::Schedule,
                ChoiceCategory::ScheduleTile,
                json!({
                    "command":"schedule.apply",
                    "request_id":format!("request-{name}-{index}"),
                    "actions":[{"kind":"tile_axes","axis":format!("sa{index}"),"factor":index + 1}]
                }),
                ChoicePreconditions {
                    bases: BTreeMap::from([("schedule_hash".to_owned(), name.to_owned())]),
                    required_capabilities: vec!["serial".to_owned()],
                    hard_conditions: vec!["production verifier acceptance required".to_owned()],
                },
                format!("choice {index}"),
                if index == 0 { "advance" } else { "unchanged" },
                format!("sa{index}"),
            )
            .unwrap()
        })
        .collect();
    build_choice_set(name, &schema, values, &RankingLimits::default()).unwrap()
}

fn label(selected: agentir_policy_eval::EvaluationChoiceId) -> RankingLabel {
    RankingLabel {
        version: 1,
        selected_choice: Some(selected),
        accepted: true,
        progress_producing: true,
        proof_frontier_delta: "advance".to_owned(),
        decisions_remaining: 0,
        repair_occurred: false,
        task_completed: true,
        semantic_task_criterion: true,
        context_bytes: 100,
        deterministic_tokens: Some(25),
        compatible_performance: None,
    }
}

fn dataset_and_split() -> (
    agentir_policy_eval::RankingDataset,
    agentir_policy_eval::DatasetSplit,
    agentir_policy_eval::FeatureSchema,
) {
    let schema = feature_schema_v1().unwrap();
    let limits = LearnedRankingLimits::default();
    let records = (0..8)
        .map(|index| {
            let choice_set = set(&format!("state-{index}"), 2);
            let input = ranking_input(
                &choice_set,
                &schema,
                FrameCompleteness::Complete,
                PolicyKind::Menu,
                false,
                BTreeMap::from([
                    ("task_id".to_owned(), format!("task-{index}")),
                    ("corpus_version".to_owned(), "1".to_owned()),
                ]),
            )
            .unwrap();
            (
                format!("episode-hash-{index}"),
                input,
                label(choice_set.choices[0].id.clone()),
            )
        })
        .collect();
    let dataset = build_dataset(
        "corpus",
        schema.feature_schema_hash.clone(),
        0,
        records,
        &limits,
    )
    .unwrap();
    let mut split = split_dataset(&dataset, 0).unwrap();
    for kind in split.groups.values_mut() {
        *kind = SplitKind::Train;
    }
    split.counts = BTreeMap::from([
        (SplitKind::Train, 8),
        (SplitKind::Validation, 0),
        (SplitKind::Test, 0),
        (SplitKind::Excluded, 0),
    ]);
    split.dataset_split_hash.clear();
    split.dataset_split_hash = dataset_split_hash(&split).unwrap();
    (dataset, split, schema)
}

#[test]
fn continuation_pages_are_exact_resumable_and_anchor_checked() {
    let choice_set = set("cursor", 5);
    let anchors = ContinuationAnchors {
        locator: "workspace:w1/schedule:s1".to_owned(),
        revisions_and_hashes: BTreeMap::from([("schedule_hash".to_owned(), "cursor".to_owned())]),
    };
    let first = paginate_choice_set(&choice_set, anchors.clone(), 2, 5, 10, None).unwrap();
    assert_eq!(first.status, ContinuationStatus::NotExhausted);
    assert_eq!(first.completeness, FrameCompleteness::Bounded);
    let second = paginate_choice_set(
        &choice_set,
        anchors.clone(),
        2,
        5,
        10,
        first.cursor.as_ref(),
    )
    .unwrap();
    let third = paginate_choice_set(
        &choice_set,
        anchors.clone(),
        2,
        5,
        10,
        second.cursor.as_ref(),
    )
    .unwrap();
    assert_eq!(third.status, ContinuationStatus::Exhausted);
    let paged = first
        .choices
        .iter()
        .chain(&second.choices)
        .chain(&third.choices)
        .map(|choice| choice.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        paged,
        choice_set
            .choices
            .iter()
            .map(|choice| choice.id.clone())
            .collect::<Vec<_>>()
    );
    let repeated = paginate_choice_set(
        &choice_set,
        anchors.clone(),
        2,
        5,
        10,
        first.cursor.as_ref(),
    )
    .unwrap();
    assert_eq!(second, repeated);
    let mut stale = anchors.clone();
    stale
        .revisions_and_hashes
        .insert("schedule_hash".to_owned(), "mutated".to_owned());
    assert_eq!(
        paginate_choice_set(&choice_set, stale, 2, 5, 10, first.cursor.as_ref())
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationContinuationStale
    );
    let corrupt = ContinuationCursor(format!("{}x", first.cursor.unwrap().0));
    assert_eq!(
        paginate_choice_set(&choice_set, anchors, 2, 5, 10, Some(&corrupt))
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationContinuationCorrupt
    );
}

#[test]
fn continuation_boundaries_cover_empty_one_exact_and_limit_plus_one() {
    let anchors = ContinuationAnchors {
        locator: "run:r1".to_owned(),
        revisions_and_hashes: BTreeMap::new(),
    };
    let empty = set("empty", 0);
    let page = paginate_choice_set(&empty, anchors.clone(), 0, 0, 1, None).unwrap();
    assert_eq!(page.status, ContinuationStatus::Exhausted);
    assert_eq!(page.completeness, FrameCompleteness::Complete);
    let one = set("one", 1);
    assert_eq!(
        paginate_choice_set(&one, anchors.clone(), 1, 1, 1, None)
            .unwrap()
            .completeness,
        FrameCompleteness::Complete
    );
    let exact = set("exact", 2);
    assert_eq!(
        paginate_choice_set(&exact, anchors.clone(), 2, 2, 2, None)
            .unwrap()
            .status,
        ContinuationStatus::Exhausted
    );
    assert_eq!(
        paginate_choice_set(&exact, anchors, 2, 1, 2, None)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationContinuationLimitExceeded
    );
}

#[test]
fn typed_repairs_cover_taxonomy_and_expire_on_anchor_change() {
    let base = BTreeMap::from([("revision".to_owned(), "r1".to_owned())]);
    let codes = [
        RepairCode::StaleBase,
        RepairCode::InvalidReference,
        RepairCode::TypeMismatch,
        RepairCode::ShapeMismatch,
        RepairCode::OpenObligation,
        RepairCode::UnsupportedRewrite,
        RepairCode::UnsafeMemoryReuse,
        RepairCode::IllegalScheduleTransform,
        RepairCode::ResourceLimit,
        RepairCode::UnsupportedBackendLowering,
        RepairCode::RankingSchemaModelMismatch,
        RepairCode::StaleContinuationCursor,
    ];
    for code in codes {
        let repair = typed_repair(
            code,
            RepairAnchor {
                diagnostic_code: "RESOURCE_LIMIT_EXCEEDED".to_owned(),
                exact_base: base.clone(),
            },
            json!({"command":"spec.apply","actions":[]}),
            "ordinary production repair",
            1,
        )
        .unwrap();
        validate_repair(&repair, &base).unwrap();
        assert_eq!(
            validate_repair(
                &repair,
                &BTreeMap::from([("revision".to_owned(), "r2".to_owned())])
            )
            .unwrap_err()
            .code,
            EvaluationErrorCode::EvaluationTypedRepairStale
        );
    }
    assert_eq!(
        typed_repair(
            RepairCode::OpenObligation,
            RepairAnchor {
                diagnostic_code: "OPEN".to_owned(),
                exact_base: base,
            },
            json!({"proof":"agent supplied"}),
            "invalid",
            1,
        )
        .unwrap_err()
        .code,
        EvaluationErrorCode::EvaluationTypedRepairInvalid
    );
}

#[test]
fn dataset_split_training_and_inference_are_byte_reproducible() {
    let (dataset, split, schema) = dataset_and_split();
    let limits = LearnedRankingLimits::default();
    validate_dataset(&dataset, &limits, true).unwrap();
    let configuration = TrainingConfiguration::v1(0).unwrap();
    let first =
        train_linear_ranker(&dataset, &split, &schema, &configuration, None, &limits).unwrap();
    let second =
        train_linear_ranker(&dataset, &split, &schema, &configuration, None, &limits).unwrap();
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
    let policy = learned_policy(&first.1, PolicyKind::Menu, false, limits.work_units).unwrap();
    let input = &dataset.examples[0].input;
    let inferred = infer(input, &schema, &first.1, &policy, &limits).unwrap();
    let repeated = infer(input, &schema, &first.1, &policy, &limits).unwrap();
    assert_eq!(inferred, repeated);
    verify_inference(input, &schema, &first.1, &policy, &inferred.1, &limits).unwrap();
    assert_eq!(inferred.1.scores.len(), 2);
    assert!(
        inferred
            .1
            .scores
            .iter()
            .all(|score| score.score.scale == FixedScore::SCALE_V1)
    );
}

#[test]
fn leakage_corruption_overflow_and_failed_inference_reject_before_output() {
    let (mut dataset, _split, _schema) = dataset_and_split();
    let limits = LearnedRankingLimits::default();
    dataset.examples[0]
        .input
        .task_metadata
        .insert("final_success".to_owned(), "true".to_owned());
    dataset.examples[0].input.ranking_input_hash.clear();
    dataset.examples[0].input.ranking_input_hash =
        ranking_input_hash(&dataset.examples[0].input).unwrap();
    assert_eq!(
        validate_dataset(&dataset, &limits, true).unwrap_err().code,
        EvaluationErrorCode::EvaluationDatasetLeakage
    );

    let (dataset, split, schema) = dataset_and_split();
    let configuration = TrainingConfiguration::v1(u64::MAX).unwrap();
    let (_, mut model) =
        train_linear_ranker(&dataset, &split, &schema, &configuration, None, &limits).unwrap();
    let policy = learned_policy(&model, PolicyKind::Menu, false, limits.work_units).unwrap();
    model.weights[0] = i64::MAX;
    assert_eq!(
        infer(
            &dataset.examples[0].input,
            &schema,
            &model,
            &policy,
            &limits
        )
        .unwrap_err()
        .code,
        EvaluationErrorCode::EvaluationModelInvalid
    );

    let mut too_many_epochs = TrainingConfiguration::v1(0).unwrap();
    too_many_epochs.epochs = limits.epochs + 1;
    too_many_epochs.training_configuration_hash.clear();
    too_many_epochs.training_configuration_hash =
        training_configuration_hash(&too_many_epochs).unwrap();
    assert_eq!(
        train_linear_ranker(&dataset, &split, &schema, &too_many_epochs, None, &limits,)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationTrainingInvalid
    );
}

#[test]
fn work_counters_are_bounded_and_excluded_from_inference_identity() {
    let (dataset, split, schema) = dataset_and_split();
    let limits = LearnedRankingLimits::default();
    let configuration = TrainingConfiguration::v1(0).unwrap();
    let (_, model) =
        train_linear_ranker(&dataset, &split, &schema, &configuration, None, &limits).unwrap();
    let policy = learned_policy(&model, PolicyKind::Menu, false, limits.work_units).unwrap();
    let (_, mut record) = infer(
        &dataset.examples[0].input,
        &schema,
        &model,
        &policy,
        &limits,
    )
    .unwrap();
    let retained = inference_hash(&record).unwrap();
    record.work_units.archive_parse = 999;
    assert_eq!(retained, inference_hash(&record).unwrap());
    assert_eq!(
        WorkUnitCounters {
            descriptor_query: u64::MAX,
            hashing: 1,
            ..WorkUnitCounters::default()
        }
        .total()
        .unwrap_err()
        .code,
        EvaluationErrorCode::EvaluationWorkUnitOverflow
    );
}

#[test]
fn learned_inference_is_read_only_until_one_explicit_production_dispatch() {
    let (dataset, split, schema) = dataset_and_split();
    let limits = LearnedRankingLimits::default();
    let configuration = TrainingConfiguration::v1(0).unwrap();
    let (_, model) =
        train_linear_ranker(&dataset, &split, &schema, &configuration, None, &limits).unwrap();
    let ranker = learned_policy(&model, PolicyKind::Menu, false, limits.work_units).unwrap();
    let policy = ranked_policy(&ranker, PolicyOrigin::Learned).unwrap();
    let mut harness = EvaluationHarness::new().unwrap();
    let run = harness
        .start_run(
            policy,
            &[EvaluationTaskId("memory-reuse-medium".to_owned())],
            &[0],
        )
        .unwrap();
    let episode = harness.run(&run).unwrap().episodes[0].id.clone();
    let observation = harness.next_observation(&episode).unwrap();
    let choice_set = harness.ranked_choice_set(&episode).unwrap();
    let input = ranking_input(
        &choice_set,
        &schema,
        FrameCompleteness::Complete,
        PolicyKind::Menu,
        false,
        BTreeMap::from([
            ("task_id".to_owned(), "memory-reuse-medium".to_owned()),
            ("corpus_version".to_owned(), "stage6a-corpus-v1".to_owned()),
        ]),
    )
    .unwrap();
    let before = harness.run(&run).unwrap().episodes[0].steps.len();
    let mut corrupt = model.clone();
    corrupt.feature_schema_hash = "wrong".to_owned();
    assert!(infer(&input, &schema, &corrupt, &ranker, &limits).is_err());
    assert_eq!(harness.run(&run).unwrap().episodes[0].steps.len(), before);

    let (decision, _) = infer(&input, &schema, &model, &ranker, &limits).unwrap();
    harness
        .rank_episode_with_descriptor(
            agentir_policy_eval::RankingSubmission {
                episode_id: episode,
                step_id: observation.step_id,
                observation_hash: observation.observation_hash,
                choice_set_hash: choice_set.choice_set_hash,
                feature_schema_hash: schema.feature_schema_hash,
                ranking_policy_hash: ranker.ranking_policy_hash.clone(),
                decision,
                usage: None,
                correlation_id: None,
            },
            &ranker,
        )
        .unwrap();
    assert_eq!(
        harness.run(&run).unwrap().episodes[0].steps.len(),
        before + 1
    );
}

#[test]
fn archive_v2_migrates_without_inventing_learning() {
    let mut harness = EvaluationHarness::new().unwrap();
    let run = harness
        .run_scripted(
            "free_reference_v1",
            &[EvaluationTaskId("saxpy-end-to-end-large".to_owned())],
            &[0],
        )
        .unwrap();
    let mut legacy = harness.archive(&[run]).unwrap();
    legacy.manifest.version = 2;
    legacy.learning_statuses.clear();
    legacy.search_history_status = agentir_policy_eval::SearchHistoryStatus::Unspecified;
    legacy.measured_search_history_status =
        agentir_policy_eval::MeasuredSearchHistoryStatus::Unspecified;
    legacy.measurement_acquisition_history_status =
        agentir_policy_eval::MeasurementAcquisitionHistoryStatus::Unspecified;
    legacy.archive_hash.clear();
    legacy.archive_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_V2_DOMAIN,
        &legacy,
    )
    .unwrap();
    verify_archive(&legacy).unwrap();
    let migrated = migrate_archive_v2_to_v3(&legacy).unwrap();
    assert_eq!(migrated.manifest.version, 3);
    assert!(migrated.ranking_datasets.is_empty());
    assert!(
        migrated
            .learning_statuses
            .values()
            .all(|status| *status == LearningEpisodeStatus::Unlearned)
    );
    verify_archive(&migrated).unwrap();
}

#[test]
fn archive_rejects_changed_model_and_inference_hashes() {
    let limits = LearnedRankingLimits::default();
    let schema = feature_schema_v1().unwrap();
    let choice_set = set("archive-state", 2);
    let mut harness = EvaluationHarness::new().unwrap();
    let run = harness
        .run_scripted(
            "free_reference_v1",
            &[EvaluationTaskId("saxpy-end-to-end-large".to_owned())],
            &[0],
        )
        .unwrap();
    let mut archive = harness.archive(&[run]).unwrap();
    archive.manifest.version = 3;
    archive.search_history_status = agentir_policy_eval::SearchHistoryStatus::Unspecified;
    archive.measured_search_history_status =
        agentir_policy_eval::MeasuredSearchHistoryStatus::Unspecified;
    archive.measurement_acquisition_history_status =
        agentir_policy_eval::MeasurementAcquisitionHistoryStatus::Unspecified;
    let episode_hash = archive.runs[0].episodes[0].episode_hash.clone().unwrap();
    archive.feature_schemas.push(schema.clone());
    archive.choice_sets.push(choice_set.clone());
    archive.archive_hash.clear();
    archive.archive_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_V3_DOMAIN,
        &archive,
    )
    .unwrap();
    verify_archive(&archive).unwrap();
    let input = ranking_input(
        &choice_set,
        &schema,
        FrameCompleteness::Complete,
        PolicyKind::Menu,
        false,
        BTreeMap::from([
            ("task_id".to_owned(), "task-0".to_owned()),
            ("corpus_version".to_owned(), "1".to_owned()),
        ]),
    )
    .unwrap();
    let dataset = build_dataset(
        archive.corpus.corpus_hash.clone(),
        schema.feature_schema_hash.clone(),
        0,
        vec![(
            episode_hash,
            input.clone(),
            label(choice_set.choices[0].id.clone()),
        )],
        &limits,
    )
    .unwrap();
    let mut split = split_dataset(&dataset, 0).unwrap();
    split.groups.insert(
        dataset.examples[0].semantic_group_hash.clone(),
        SplitKind::Train,
    );
    split.counts = BTreeMap::from([
        (SplitKind::Train, 1),
        (SplitKind::Validation, 0),
        (SplitKind::Test, 0),
        (SplitKind::Excluded, 0),
    ]);
    split.dataset_split_hash.clear();
    split.dataset_split_hash = dataset_split_hash(&split).unwrap();
    let configuration = TrainingConfiguration::v1(0).unwrap();
    let (training, model) =
        train_linear_ranker(&dataset, &split, &schema, &configuration, None, &limits).unwrap();
    let policy = learned_policy(&model, PolicyKind::Menu, false, limits.work_units).unwrap();
    let (_, inference) = infer(&input, &schema, &model, &policy, &limits).unwrap();
    let archive = agentir_policy_eval::migrate_archive_v3_to_v4(&archive).unwrap();
    let learned = attach_learning_artifacts(
        &archive,
        LearnedArchiveBundle {
            datasets: vec![dataset],
            splits: vec![split],
            configurations: vec![configuration],
            training_runs: vec![training],
            models: vec![model],
            inputs: vec![input],
            inferences: vec![inference],
            policies: vec![policy],
        },
    )
    .unwrap();
    verify_archive(&learned).unwrap();

    let mut changed_weight = learned.clone();
    changed_weight.learned_models[0].weights[0] += 1;
    changed_weight.archive_hash.clear();
    changed_weight.archive_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_V4_DOMAIN,
        &changed_weight,
    )
    .unwrap();
    assert_eq!(
        verify_archive(&changed_weight).unwrap_err().code,
        EvaluationErrorCode::EvaluationModelInvalid
    );

    let mut changed_score = learned;
    changed_score.inference_records[0].scores[0].score.units += 1;
    changed_score.archive_hash.clear();
    changed_score.archive_hash = agentir_policy_eval::hashing::domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_V4_DOMAIN,
        &changed_score,
    )
    .unwrap();
    assert_eq!(
        verify_archive(&changed_score).unwrap_err().code,
        EvaluationErrorCode::EvaluationInferenceInvalid
    );
}
