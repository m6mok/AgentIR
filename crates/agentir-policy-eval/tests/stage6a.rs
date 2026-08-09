use agentir_policy_eval::{
    EvaluationErrorCode, EvaluationHarness, EvaluationLimits, EvaluationProtocol, EvaluationTaskId,
    PolicyDecision, PolicyKind, UsageTrust, builtin_corpus, external_policy,
    hashing::{CORPUS_HASH_DOMAIN, POLICY_HASH_DOMAIN, domain_hash},
    scripted_policy, verify_archive,
};
use serde_json::{Value, json};

fn saxpy() -> [EvaluationTaskId; 1] {
    [EvaluationTaskId("saxpy-end-to-end-large".to_owned())]
}

#[test]
fn corpus_and_policy_hashes_are_deterministic_and_domain_separated() {
    let first = builtin_corpus().unwrap();
    let second = builtin_corpus().unwrap();
    assert_eq!(first.corpus_hash, second.corpus_hash);
    assert_eq!(first.tasks.len(), 20);

    let mut reordered = first.clone();
    reordered.tasks.swap(0, 1);
    reordered.corpus_hash.clear();
    assert_ne!(
        first.corpus_hash,
        domain_hash(CORPUS_HASH_DOMAIN, &reordered).unwrap()
    );

    let first_policy = scripted_policy("free_reference_v1").unwrap();
    let mut changed = first_policy.clone();
    changed.policy_hash.clear();
    changed.configuration.insert("variant".to_owned(), json!(2));
    assert_ne!(
        first_policy.policy_hash,
        domain_hash(POLICY_HASH_DOMAIN, &changed).unwrap()
    );
}

#[test]
fn free_menu_and_hybrid_expose_distinct_bounded_surfaces() {
    for (kind, choices, schema) in [
        (PolicyKind::Free, 0, "production_request"),
        (PolicyKind::Menu, 1, "menu_choice"),
        (PolicyKind::Hybrid, 1, "hybrid"),
    ] {
        let mut harness = EvaluationHarness::new().unwrap();
        let policy = external_policy(kind, "test").unwrap();
        let run = harness.start_run(policy, &saxpy(), &[7]).unwrap();
        let episode = harness.run(&run).unwrap().episodes[0].id.clone();
        let observation = harness.next_observation(&episode).unwrap();
        assert_eq!(observation.continuation_frame.len(), choices);
        assert!(match schema {
            "hybrid" => observation.allowed_action_schema.get("one_of").is_some(),
            expected => observation.allowed_action_schema["kind"] == expected,
        });
        assert!(observation.open_obligations.is_empty());
    }
}

#[test]
fn menu_violation_and_stale_observation_are_atomic() {
    let mut harness = EvaluationHarness::new().unwrap();
    let run = harness
        .start_run(
            external_policy(PolicyKind::Menu, "menu").unwrap(),
            &saxpy(),
            &[0],
        )
        .unwrap();
    let episode = harness.run(&run).unwrap().episodes[0].id.clone();
    let observation = harness.next_observation(&episode).unwrap();
    let error = harness
        .submit(
            &episode,
            &observation.step_id,
            &observation.observation_hash,
            PolicyDecision::Action {
                action: json!({"command": "workspace.open", "request_id": "x"}),
                escape: false,
            },
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(error.code, EvaluationErrorCode::EvaluationPolicyViolation);
    assert!(harness.run(&run).unwrap().episodes[0].steps.is_empty());

    let error = harness
        .submit(
            &episode,
            &observation.step_id,
            "00",
            PolicyDecision::MenuChoice {
                choice_id: observation.continuation_frame[0].choice_id.clone(),
            },
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(
        error.code,
        EvaluationErrorCode::EvaluationObservationMismatch
    );
    assert!(harness.run(&run).unwrap().episodes[0].steps.is_empty());
}

#[test]
fn scripted_baselines_are_successful_replayable_and_comparable() {
    let mut harness = EvaluationHarness::new().unwrap();
    let mut runs = Vec::new();
    for policy in [
        "free_reference_v1",
        "menu_first_valid_v1",
        "menu_goal_directed_v1",
        "hybrid_menu_preferred_v1",
        "hybrid_bounded_escape_v1",
    ] {
        let run = harness.run_scripted(policy, &saxpy(), &[42]).unwrap();
        harness.replay_run(&run).unwrap();
        let aggregate = harness.aggregate(&run).unwrap();
        assert!((aggregate.success_rate - 1.0).abs() < f64::EPSILON);
        runs.push(run);
    }
    let comparison = harness.compare(&runs).unwrap();
    assert_eq!(comparison.aggregates.len(), 5);
}

#[test]
fn golden_stage6a_episode_hashes_are_pinned() {
    let cases = [
        (
            "free_reference_v1",
            "saxpy-end-to-end-large",
            "301488f5e4c2e11c523cc2810718e115324c62b0ec9600d443713470f8411d39",
        ),
        (
            "menu_goal_directed_v1",
            "saxpy-end-to-end-large",
            "1d961b31a70acd9755743482438b286eaf0f60cd5e1ea48bc4131753bb37b252",
        ),
        (
            "hybrid_bounded_escape_v1",
            "saxpy-end-to-end-large",
            "ba403c0cdb7d03bd87cb36dc1c0f1c1e6a0157ee5adf459088aab89b8bffa430",
        ),
        (
            "free_reference_v1",
            "backend-unsupported-small",
            "062ecbef4805c25d1d0b052528607846f663e73eb3a1ce474c30b8047f7a5c24",
        ),
        (
            "menu_first_valid_v1",
            "candidate-rewrite-small",
            "5f7d6475de231da09e3fb322e3d30adb72ba358750b2b878a18950fb1532ec91",
        ),
        (
            "hybrid_menu_preferred_v1",
            "memory-guarded-medium",
            "61ad52343c8fbbd92b28336cb440a2d031ef76f69889ebfddaa50333f9fdfb22",
        ),
        (
            "menu_goal_directed_v1",
            "schedule-tiled-medium",
            "1626c368ac48b67f2ec94ce2f15746939cd4e147061350f5fb179b087f7633b7",
        ),
        (
            "hybrid_menu_preferred_v1",
            "backend-lowering-medium",
            "cc431010c62536ab41e0e9f21acf03a8e23407bbc2e3986c16d86c40dd0703a5",
        ),
        (
            "free_reference_v1",
            "backend-unsupported-small",
            "062ecbef4805c25d1d0b052528607846f663e73eb3a1ce474c30b8047f7a5c24",
        ),
        (
            "menu_first_valid_v1",
            "equality-artifact-large",
            "765fda642ba7cb39b555642bb414f7ed7f55e4860dd5409f68d8ef866f2eebf1",
        ),
    ];
    for (policy, task, expected) in cases {
        let mut harness = EvaluationHarness::new().unwrap();
        let run = harness
            .run_scripted(policy, &[EvaluationTaskId(task.to_owned())], &[0])
            .unwrap();
        assert_eq!(
            harness.run(&run).unwrap().episodes[0]
                .episode_hash
                .as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn unknown_and_provider_token_accounting_remain_distinct() {
    let mut harness = EvaluationHarness::new().unwrap();
    let run = harness
        .start_run(
            external_policy(PolicyKind::Free, "external").unwrap(),
            &saxpy(),
            &[0],
        )
        .unwrap();
    let episode = harness.run(&run).unwrap().episodes[0].id.clone();
    let observation = harness.next_observation(&episode).unwrap();
    let task = harness.task(&observation.task_id).unwrap();
    harness
        .submit(
            &episode,
            &observation.step_id,
            &observation.observation_hash,
            PolicyDecision::Action {
                action: task.initial_state.production_requests[0].clone(),
                escape: false,
            },
            Some(agentir_policy_eval::TokenUsage {
                input_tokens: Some(10),
                output_tokens: None,
                trust: UsageTrust::ProviderReported,
                tokenizer: Some("opaque-provider".to_owned()),
            }),
            None,
        )
        .unwrap();
    let step = &harness.run(&run).unwrap().episodes[0].steps[0];
    assert_eq!(
        step.context.reported_usage.as_ref().unwrap().output_tokens,
        None
    );
    assert_eq!(
        step.context.reported_usage.as_ref().unwrap().trust,
        UsageTrust::ProviderReported
    );
}

#[test]
fn evaluation_archive_verifies_and_mutation_is_rejected_without_panic() {
    let mut harness = EvaluationHarness::new().unwrap();
    let run = harness
        .run_scripted("free_reference_v1", &saxpy(), &[0])
        .unwrap();
    let archive = harness.archive(&[run]).unwrap();
    verify_archive(&archive).unwrap();
    let path = std::env::temp_dir().join(format!(
        "agentir-stage6a-archive-{}.json",
        std::process::id()
    ));
    let run_id = archive.runs[0].id.clone();
    harness.save_archive(&path, &[run_id]).unwrap();
    let loaded = EvaluationHarness::load_archive(&path, &EvaluationLimits::default()).unwrap();
    assert_eq!(loaded.archive_hash, archive.archive_hash);
    let mut imported = EvaluationHarness::new().unwrap();
    imported.import_archive(&path).unwrap();
    imported.replay_run(&archive.runs[0].id).unwrap();
    std::fs::remove_file(&path).unwrap();

    let mut mutated = archive;
    mutated.corpus.tasks.swap(0, 1);
    assert!(verify_archive(&mutated).is_err());
}

#[test]
fn limit_plus_one_rejects_before_run_publication() {
    let corpus = builtin_corpus().unwrap();
    let limits = EvaluationLimits {
        runs: 0,
        ..EvaluationLimits::default()
    };
    let mut harness = EvaluationHarness::with_corpus_and_limits(corpus, limits).unwrap();
    let error = harness
        .start_run(
            scripted_policy("free_reference_v1").unwrap(),
            &saxpy(),
            &[0],
        )
        .unwrap_err();
    assert_eq!(error.code, EvaluationErrorCode::EvaluationBudgetExceeded);
    assert_eq!(harness.run_ids().count(), 0);
}

#[test]
fn jsonl_has_one_response_per_line_and_agent_cannot_supply_success() {
    let mut protocol = EvaluationProtocol::new().unwrap();
    let line = protocol.process_line(r#"{"command":"evaluation.corpus.list","request_id":"list"}"#);
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["ok"], true);
    assert_eq!(line.lines().count(), 1);

    let rejected = protocol.process_line(
        r#"{"command":"evaluation.episode.submit","request_id":"bad","run":"run1","episode":"ep1","step":"step0","observation_hash":"x","decision":{"kind":"finish"},"success":true}"#,
    );
    let response: Value = serde_json::from_str(&rejected).unwrap();
    assert_eq!(response["ok"], false);
}
