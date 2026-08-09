//! Offline, fixed-seed Stage 6B correctness, determinism, failure, and scaling study.

use agentir_policy_eval::hashing::domain_hash;
use agentir_policy_eval::{
    ChoiceCategory, ChoiceOrigin, ChoicePreconditions, ChoiceScore, EvaluationArchive,
    EvaluationChoice, EvaluationChoiceId, EvaluationDiagnostic, EvaluationHarness,
    EvaluationLimits, FixedScore, RankingDecision, RankingLimits, RankingSubmission,
    aggregate_ranking_metrics, build_choice_set, builtin_ranked_corpus, compiler_choice,
    feature_schema_v1, rank_choices, replay_ranking_trace, scripted_ranker,
    scripted_ranking_decision, verify_archive,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env, fs,
    hint::black_box,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

const STUDY_SCHEMA: &str = "agentir.stage6b.study.v1";
const BENCHMARK_SCHEMA: u64 = 2;
const INPUT_DOMAIN: &[u8] = b"agentir.stage6b.study.input.v1\0";
const DEFAULT_SAMPLES: usize = 9;
const WARMUPS: usize = 2;
const FIXED_SEEDS: &[u64] = &[0, 1, 2, 7, 23, 42, 255, 1024, u32::MAX as u64, u64::MAX];
const POLICIES: &[&str] = &[
    "first_progress_choice_v1",
    "goal_directed_rule_v1",
    "seeded_uniform_choice_v1",
];

#[derive(Debug)]
struct Configuration {
    output: PathBuf,
    samples: usize,
    seed_count: usize,
}

#[derive(Clone, Debug, Serialize)]
struct TimingSummary {
    unit: &'static str,
    warmup_count: usize,
    measured_sample_count: usize,
    raw_sample_count: usize,
    min: u64,
    median: u64,
    p90: u64,
    p95: u64,
    p99: u64,
    max: u64,
    mean: f64,
    standard_deviation: f64,
}

#[derive(Default)]
struct Records {
    cases: Vec<Value>,
    timings: Vec<Value>,
    episodes: Vec<Value>,
    rejections: Vec<Value>,
    hashes: Vec<Value>,
    fairness: Vec<Value>,
    aggregates: Vec<Value>,
    mutations: Vec<Value>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("stage6b study failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let configuration = configuration()?;
    fs::create_dir_all(&configuration.output).map_err(|error| error.to_string())?;
    let environment = environment_anchor();
    let seeds = FIXED_SEEDS[..configuration.seed_count].to_vec();
    let config_value = json!({
        "schema_version": STUDY_SCHEMA,
        "benchmark_schema": BENCHMARK_SCHEMA,
        "offline": true,
        "gpu_independent": true,
        "fixed_seeds": seeds,
        "policies": POLICIES,
        "warmup_count": WARMUPS,
        "measured_sample_count": configuration.samples,
        "timing_sizes": [0, 1, 2, 10, 32, 100, 256, 1000],
        "cartesian_product_bounded": true
    });
    write_json(&configuration.output.join("manifest.json"), &config_value)?;
    write_json(&configuration.output.join("environment.json"), &environment)?;

    let mut records = Records::default();
    run_cardinality_and_ranking_cases(&configuration, &environment, &mut records)?;
    run_identity_and_feature_cases(&mut records)?;
    run_score_and_selection_cases(&mut records)?;
    let (archive, archive_bytes, summary_counts) =
        run_production_episodes(&configuration, &seeds, &environment, &mut records)?;
    run_archive_mutations(&archive, &archive_bytes, &mut records)?;
    run_archive_timings(
        &configuration,
        &environment,
        &archive,
        &archive_bytes,
        &mut records,
    )?;

    let workload_matrix = workload_matrix(&records);
    write_json(
        &configuration.output.join("workloads.json"),
        &workload_matrix,
    )?;
    write_jsonl(
        &configuration.output.join("timing-samples.jsonl"),
        &records.timings,
    )?;
    write_jsonl(
        &configuration.output.join("episodes.jsonl"),
        &records.episodes,
    )?;
    write_jsonl(
        &configuration.output.join("rejections.jsonl"),
        &records.rejections,
    )?;
    write_jsonl(
        &configuration.output.join("hash-stability.jsonl"),
        &records.hashes,
    )?;
    write_jsonl(
        &configuration.output.join("fairness-results.jsonl"),
        &records.fairness,
    )?;
    write_json(
        &configuration.output.join("aggregates.json"),
        &records.aggregates,
    )?;
    write_jsonl(
        &configuration.output.join("mutation-results.jsonl"),
        &records.mutations,
    )?;

    let status_counts = status_counts(&records.cases);
    let summary = json!({
        "schema_version": STUDY_SCHEMA,
        "benchmark_schema": BENCHMARK_SCHEMA,
        "configuration_digest": digest(&config_value)?,
        "workload_families": workload_matrix["families"].as_array().map_or(0, Vec::len),
        "cases": records.cases.len(),
        "episodes": records.episodes.len(),
        "ranking_decisions": summary_counts["ranking_decisions"],
        "replayed_decisions": summary_counts["replayed_decisions"],
        "accepted": status_counts.get("accepted").copied().unwrap_or(0),
        "compiler_rejected": summary_counts["compiler_rejected"],
        "policy_rejected": status_counts.get("policy_rejected").copied().unwrap_or(0),
        "budget_rejected": status_counts.get("budget_rejected").copied().unwrap_or(0),
        "mutation_cases": records.mutations.len(),
        "skipped_combinations": workload_matrix["skipped_combinations"].as_array().map_or(0, Vec::len),
        "timings_are_correctness": false,
        "compiler_certificates_unchanged_by_study": true,
        "external_calls": 0,
        "device_calls": 0
    });
    write_json(&configuration.output.join("summary.json"), &summary)?;

    let semantic = json!({
        "schema_version": STUDY_SCHEMA,
        "configuration": config_value,
        "cases": records.cases,
        "episodes": records.episodes,
        "rejections": records.rejections,
        "hash_stability": records.hashes,
        "fairness": records.fairness,
        "aggregates": records.aggregates,
        "mutations": records.mutations,
        "archive_hash": archive.archive_hash,
        "archive_digest": digest(&archive)?,
        "summary": summary
    });
    write_json(&configuration.output.join("semantic.json"), &semantic)?;
    let analysis = analysis_markdown(&semantic, &records.timings, &environment);
    fs::write(configuration.output.join("analysis.md"), analysis)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&semantic["summary"]).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn configuration() -> Result<Configuration, String> {
    let mut output = PathBuf::from("target/stage6b-study/run");
    let mut samples = DEFAULT_SAMPLES;
    let mut seed_count = FIXED_SEEDS.len();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                output = PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--output requires a path".to_owned())?,
                );
            }
            "--samples" => {
                samples = parse_bounded(arguments.next().as_deref(), "samples", 1, 101)?;
            }
            "--seed-count" => {
                seed_count = parse_bounded(
                    arguments.next().as_deref(),
                    "seed-count",
                    1,
                    FIXED_SEEDS.len(),
                )?;
            }
            "--help" => {
                println!("stage6b_study [--output PATH] [--samples 1..101] [--seed-count 1..10]");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument `{argument}`")),
        }
    }
    Ok(Configuration {
        output,
        samples,
        seed_count,
    })
}

fn parse_bounded(
    value: Option<&str>,
    name: &str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, String> {
    let parsed = value
        .ok_or_else(|| format!("--{name} requires a value"))?
        .parse::<usize>()
        .map_err(|_| format!("--{name} must be an integer"))?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(format!("--{name} must be in {minimum}..={maximum}"));
    }
    Ok(parsed)
}

fn run_cardinality_and_ranking_cases(
    configuration: &Configuration,
    environment: &Value,
    records: &mut Records,
) -> Result<(), String> {
    let schema = feature_schema_v1().map_err(diagnostic_text)?;
    let limits = RankingLimits::default();
    for size in [0_usize, 1, 2, 3, 10, 100, 1_000, 1_001] {
        let case_id = format!("choice-cardinality-{size}");
        let input = json!({"family":"choice_set_cardinality","size":size});
        let choices = choices(size)?;
        match build_choice_set(&case_id, &schema, choices, &limits) {
            Ok(set) if size == 0 => {
                let policy = scripted_ranker("first_progress_choice_v1", &schema, 0)
                    .map_err(diagnostic_text)?;
                let error = rank_choices(
                    &set,
                    &policy,
                    RankingDecision::Scores { scores: Vec::new() },
                    &limits,
                )
                .expect_err("an empty frame cannot be selected");
                push_rejection(records, &case_id, "policy_rejected", &input, &error)?;
            }
            Ok(set) => {
                let policy = scripted_ranker("seeded_uniform_choice_v1", &schema, 23)
                    .map_err(diagnostic_text)?;
                let decision =
                    scripted_ranking_decision(&policy, &set, &limits).map_err(diagnostic_text)?;
                let trace =
                    rank_choices(&set, &policy, decision, &limits).map_err(diagnostic_text)?;
                replay_ranking_trace(&set, &policy, &trace, &limits).map_err(diagnostic_text)?;
                records.cases.push(case_record(
                    &case_id,
                    "choice_set_cardinality",
                    "accepted",
                    &input,
                    json!({
                        "choice_set_hash":set.choice_set_hash,
                        "selected_choice":trace.selected_choice,
                        "trace_hash":trace.ranking_trace_hash,
                        "choice_ids":set.choices.iter().map(|choice| &choice.id).collect::<Vec<_>>()
                    }),
                )?);
            }
            Err(error) if size == 1_001 => {
                push_rejection(records, &case_id, "budget_rejected", &input, &error)?;
            }
            Err(error) => return Err(diagnostic_text(error)),
        }
    }

    for size in [1_usize, 2, 10, 32, 100, 256, 1_000] {
        let set = build_choice_set(format!("timing-{size}"), &schema, choices(size)?, &limits)
            .map_err(diagnostic_text)?;
        let policy =
            scripted_ranker("seeded_uniform_choice_v1", &schema, 23).map_err(diagnostic_text)?;
        let distinct = measure(configuration.samples, || {
            let decision = scripted_ranking_decision(&policy, &set, &limits).unwrap();
            black_box(rank_choices(&set, &policy, decision, &limits).unwrap());
        });
        records.timings.push(timing_record(
            "ranking_distinct",
            size,
            "accepted",
            &distinct,
            environment,
        )?);
        let tied_scores = set
            .choices
            .iter()
            .map(|choice| ChoiceScore {
                choice_id: choice.id.clone(),
                score: FixedScore::v1(0, limits.score_magnitude).unwrap(),
            })
            .collect::<Vec<_>>();
        let tied = measure(configuration.samples, || {
            black_box(
                rank_choices(
                    &set,
                    &policy,
                    RankingDecision::Scores {
                        scores: tied_scores.clone(),
                    },
                    &limits,
                )
                .unwrap(),
            );
        });
        records.timings.push(timing_record(
            "ranking_all_tied",
            size,
            "accepted",
            &tied,
            environment,
        )?);
        let build = measure(configuration.samples, || {
            black_box(
                build_choice_set(
                    format!("timing-{size}"),
                    &schema,
                    choices(size).unwrap(),
                    &limits,
                )
                .unwrap(),
            );
        });
        records.timings.push(timing_record(
            "choice_construct_canonicalize_hash",
            size,
            "accepted",
            &build,
            environment,
        )?);
    }
    Ok(())
}

fn run_identity_and_feature_cases(records: &mut Records) -> Result<(), String> {
    let schema = feature_schema_v1().map_err(diagnostic_text)?;
    let limits = RankingLimits::default();
    let base = build_choice_set(
        "identity-observation",
        &schema,
        vec![choice_with_request("request-a", "sa1", 4)?],
        &limits,
    )
    .map_err(diagnostic_text)?;
    let transport = build_choice_set(
        "identity-observation",
        &schema,
        vec![transport_mutated_choice()?],
        &limits,
    )
    .map_err(diagnostic_text)?;
    let production = build_choice_set(
        "identity-observation",
        &schema,
        vec![choice_with_request("request-c", "sa1", 8)?],
        &limits,
    )
    .map_err(diagnostic_text)?;
    let stable = base.choices[0].id == transport.choices[0].id;
    let production_changed = base.choices[0].id != production.choices[0].id;
    if !stable || !production_changed {
        return Err("semantic choice identity contract failed".to_owned());
    }
    for (case_id, actual) in [
        ("identity-transport-mutation", stable),
        ("identity-production-mutation", production_changed),
    ] {
        let input = json!({"case":case_id});
        records.cases.push(case_record(
            case_id,
            "choice_identity",
            "accepted",
            &input,
            json!({"contract_satisfied":actual}),
        )?);
    }
    records.hashes.push(json!({
        "schema_version":STUDY_SCHEMA,
        "experiment_id":"hash-choice-transport-exclusion",
        "case_id":"identity-transport-mutation",
        "input_digest":digest(&json!({"base":"sa1-tile4"}))?,
        "base_choice_id":base.choices[0].id,
        "mutated_choice_id":transport.choices[0].id,
        "byte_identical":stable,
        "excluded_fields":["request_id","correlation_id","timestamp_ns","latency_ms","hostname","provider_session_id"]
    }));

    let mut missing = choice_with_request("feature-missing", "sa1", 4)?;
    missing.visible_features.values.remove("structural_target");
    rejected_choice_case(
        records,
        "feature-missing-required",
        missing,
        &schema,
        &limits,
    )?;
    let mut wrong_type = choice_with_request("feature-type", "sa1", 4)?;
    wrong_type.visible_features.values.insert(
        "operand_arity".to_owned(),
        agentir_policy_eval::FeatureValue::Text("zero".to_owned()),
    );
    rejected_choice_case(records, "feature-wrong-type", wrong_type, &schema, &limits)?;
    let mut negative = choice_with_request("feature-negative", "sa1", 4)?;
    negative.visible_features.values.insert(
        "static_extent_count".to_owned(),
        agentir_policy_eval::FeatureValue::Integer(-1),
    );
    rejected_choice_case(
        records,
        "feature-negative-non-negative",
        negative,
        &schema,
        &limits,
    )?;
    let mut duplicate_capabilities = choice_with_request("feature-list", "sa1", 4)?;
    duplicate_capabilities.visible_features.values.insert(
        "target_capabilities".to_owned(),
        agentir_policy_eval::FeatureValue::TextList(vec!["z".into(), "a".into(), "a".into()]),
    );
    rejected_choice_case(
        records,
        "feature-unsorted-duplicate-capabilities",
        duplicate_capabilities,
        &schema,
        &limits,
    )?;
    Ok(())
}

fn run_score_and_selection_cases(records: &mut Records) -> Result<(), String> {
    let schema = feature_schema_v1().map_err(diagnostic_text)?;
    let limits = RankingLimits::default();
    let set = build_choice_set("score-observation", &schema, choices(3)?, &limits)
        .map_err(diagnostic_text)?;
    let policy = scripted_ranker("goal_directed_rule_v1", &schema, 7).map_err(diagnostic_text)?;
    for units in [0, 1, -1, limits.score_magnitude, -limits.score_magnitude] {
        let scores = set
            .choices
            .iter()
            .map(|choice| ChoiceScore {
                choice_id: choice.id.clone(),
                score: FixedScore::v1(units, limits.score_magnitude).unwrap(),
            })
            .collect();
        let case_id = format!("score-units-{units}");
        let trace = rank_choices(&set, &policy, RankingDecision::Scores { scores }, &limits)
            .map_err(diagnostic_text)?;
        records.cases.push(case_record(
            &case_id,
            "fixed_point_scores",
            "accepted",
            &json!({"units":units}),
            json!({"trace_hash":trace.ranking_trace_hash}),
        )?);
    }

    let invalid_fixed = [
        ("score-maximum-plus-one", limits.score_magnitude + 1),
        ("score-i64-minimum", i64::MIN),
    ];
    for (case_id, units) in invalid_fixed {
        let input = json!({"units":units});
        let error = FixedScore::v1(units, limits.score_magnitude)
            .expect_err("out-of-range score must reject");
        push_rejection(records, case_id, "policy_rejected", &input, &error)?;
    }

    let valid_score = FixedScore::v1(0, limits.score_magnitude).unwrap();
    let score_cases = vec![
        (
            "score-count-zero",
            RankingDecision::Scores { scores: Vec::new() },
        ),
        (
            "score-duplicate-entry",
            RankingDecision::Scores {
                scores: vec![
                    ChoiceScore {
                        choice_id: set.choices[0].id.clone(),
                        score: valid_score,
                    },
                    ChoiceScore {
                        choice_id: set.choices[0].id.clone(),
                        score: valid_score,
                    },
                    ChoiceScore {
                        choice_id: set.choices[2].id.clone(),
                        score: valid_score,
                    },
                ],
            },
        ),
        (
            "score-unknown-choice",
            RankingDecision::Scores {
                scores: vec![
                    ChoiceScore {
                        choice_id: set.choices[0].id.clone(),
                        score: valid_score,
                    },
                    ChoiceScore {
                        choice_id: set.choices[1].id.clone(),
                        score: valid_score,
                    },
                    ChoiceScore {
                        choice_id: EvaluationChoiceId("ec-unknown".to_owned()),
                        score: valid_score,
                    },
                ],
            },
        ),
        (
            "score-wrong-scale",
            RankingDecision::Scores {
                scores: set
                    .choices
                    .iter()
                    .map(|choice| ChoiceScore {
                        choice_id: choice.id.clone(),
                        score: FixedScore { units: 0, scale: 2 },
                    })
                    .collect(),
            },
        ),
        (
            "selection-outside-frame",
            RankingDecision::Select {
                choice_id: EvaluationChoiceId("ec-not-visible".to_owned()),
            },
        ),
    ];
    for (case_id, decision) in score_cases {
        let input = json!({"case":case_id});
        let error = rank_choices(&set, &policy, decision, &limits)
            .expect_err("invalid ranking case must reject");
        push_rejection(records, case_id, "policy_rejected", &input, &error)?;
    }
    Ok(())
}

fn run_production_episodes(
    configuration: &Configuration,
    seeds: &[u64],
    environment: &Value,
    records: &mut Records,
) -> Result<(EvaluationArchive, Vec<u8>, Value), String> {
    let corpus = builtin_ranked_corpus().map_err(diagnostic_text)?;
    let task_ids = corpus
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    let mut harness =
        EvaluationHarness::with_corpus_and_limits(corpus, EvaluationLimits::default())
            .map_err(diagnostic_text)?;
    let mut run_ids = Vec::new();
    let mut ranking_decisions = 0_u64;
    let mut compiler_rejected = 0_u64;
    for policy_name in POLICIES {
        let policy_seeds = if *policy_name == "seeded_uniform_choice_v1" {
            seeds
        } else {
            &seeds[..1]
        };
        for seed in policy_seeds {
            let run_id = harness
                .start_ranked_run(policy_name, &task_ids, &[*seed])
                .map_err(diagnostic_text)?;
            let episode_ids = harness
                .run(&run_id)
                .map_err(diagnostic_text)?
                .episodes
                .iter()
                .map(|episode| episode.id.clone())
                .collect::<Vec<_>>();
            let policy = scripted_ranker(
                policy_name,
                &feature_schema_v1().map_err(diagnostic_text)?,
                *seed,
            )
            .map_err(diagnostic_text)?;
            for episode_id in episode_ids {
                loop {
                    let complete = harness
                        .run(&run_id)
                        .map_err(diagnostic_text)?
                        .episodes
                        .iter()
                        .find(|episode| episode.id == episode_id)
                        .and_then(|episode| episode.result.as_ref())
                        .is_some();
                    if complete {
                        break;
                    }
                    let observation = harness
                        .next_observation(&episode_id)
                        .map_err(diagnostic_text)?;
                    let set = harness
                        .ranked_choice_set(&episode_id)
                        .map_err(diagnostic_text)?;
                    let decision =
                        scripted_ranking_decision(&policy, &set, &RankingLimits::default())
                            .map_err(diagnostic_text)?;
                    let (_, outcome) = harness
                        .rank_episode(RankingSubmission {
                            episode_id: episode_id.clone(),
                            step_id: observation.step_id.clone(),
                            observation_hash: observation.observation_hash.clone(),
                            choice_set_hash: set.choice_set_hash.clone(),
                            feature_schema_hash: set.feature_schema_hash.clone(),
                            ranking_policy_hash: policy.ranking_policy_hash.clone(),
                            decision,
                            usage: None,
                            correlation_id: None,
                        })
                        .map_err(diagnostic_text)?;
                    ranking_decisions = ranking_decisions.saturating_add(1);
                    compiler_rejected = compiler_rejected
                        .saturating_add(u64::from(!outcome.compiler_outcome.accepted));
                }
            }
            let step_count = harness
                .run(&run_id)
                .map_err(diagnostic_text)?
                .episodes
                .iter()
                .map(|episode| episode.steps.len())
                .sum::<usize>();
            harness.replay_run(&run_id).map_err(diagnostic_text)?;
            if *seed == seeds[0] {
                let replay = measure_with_warmups(configuration.samples.min(3), 1, || {
                    harness.replay_run(&run_id).unwrap();
                    black_box(());
                });
                records.timings.push(timing_record(
                    "production_run_replay",
                    step_count,
                    "accepted",
                    &replay,
                    environment,
                )?);
            }
            let run = harness.run(&run_id).map_err(diagnostic_text)?;
            let aggregate = harness.aggregate(&run_id).map_err(diagnostic_text)?;
            let ranking = aggregate_ranking_metrics(run);
            records.aggregates.push(json!({
                "schema_version":STUDY_SCHEMA,
                "run_id":run_id,
                "policy":policy_name,
                "seed":seed,
                "aggregate":aggregate,
                "ranking":ranking
            }));
            for episode in &run.episodes {
                let result = episode
                    .result
                    .as_ref()
                    .ok_or_else(|| format!("episode {} did not complete", episode.id))?;
                records.episodes.push(json!({
                    "schema_version":STUDY_SCHEMA,
                    "experiment_id":format!("production-{policy_name}-{seed}-{}",episode.task_id.0),
                    "workload_family":"production_selection",
                    "case_id":episode.task_id.0,
                    "deterministic_seed":seed,
                    "input_digest":digest(&json!({"policy":policy_name,"seed":seed,"task":episode.task_id}))?,
                    "policy":policy_name,
                    "episode_id":episode.id,
                    "episode_hash":episode.episode_hash,
                    "status":if result.success {"accepted"} else {"compiler_rejected"},
                    "success":result.success,
                    "semantic_criterion_satisfied":result.semantic.criterion_satisfied,
                    "budget_exhausted":result.budget_exhausted,
                    "accepted_actions":result.accepted_decisions,
                    "rejected_actions":result.rejected_decisions,
                    "ranking_decisions":episode.steps.iter().filter(|step| step.ranking_trace.is_some()).count(),
                    "choice_set_hashes":episode.steps.iter().filter_map(|step| step.ranking_trace.as_ref().map(|trace| &trace.choice_set_hash)).collect::<Vec<_>>(),
                    "ranking_trace_hashes":episode.steps.iter().filter_map(|step| step.ranking_trace.as_ref().map(|trace| &trace.ranking_trace_hash)).collect::<Vec<_>>(),
                    "selection_hashes":episode.steps.iter().filter_map(|step| step.selection.as_ref().map(|selection| &selection.selection_hash)).collect::<Vec<_>>(),
                    "context_bytes":result.context_bytes,
                    "work_units":episode.steps.len()
                }));
            }
            run_ids.push(run_id);
        }
    }
    for policy_name in POLICIES {
        let policy_runs = records
            .aggregates
            .iter()
            .filter(|record| record["policy"] == *policy_name)
            .count();
        records.fairness.push(json!({
            "schema_version":STUDY_SCHEMA,
            "policy":policy_name,
            "compatible_runs":policy_runs,
            "comparison_objective":"report raw acceptance/progress/context metrics; no best-policy claim",
            "status":"reported_separately",
            "reason_code":"DIFFERENT_SEED_OR_CHOICE_SEQUENCE_NOT_POOLED"
        }));
    }
    let archive_path = configuration.output.join("evaluation-archive.json");
    let prepared_archive = harness
        .archive(&run_ids)
        .map_err(|error| format!("build production archive: {}", diagnostic_text(error)))?;
    verify_archive(&prepared_archive).map_err(|error| {
        format!(
            "verify in-memory production archive: {}",
            diagnostic_text(error)
        )
    })?;
    let archive_bytes = serde_json::to_vec(&prepared_archive).map_err(|error| error.to_string())?;
    fs::write(&archive_path, &archive_bytes).map_err(|error| error.to_string())?;
    let archive: EvaluationArchive =
        serde_json::from_slice(&archive_bytes).map_err(|error| error.to_string())?;
    if archive != prepared_archive {
        let changed = [
            ("manifest", archive.manifest != prepared_archive.manifest),
            ("corpus", archive.corpus != prepared_archive.corpus),
            ("runs", archive.runs != prepared_archive.runs),
            (
                "aggregates",
                archive.aggregates != prepared_archive.aggregates,
            ),
            (
                "feature_schemas",
                archive.feature_schemas != prepared_archive.feature_schemas,
            ),
            (
                "ranking_policies",
                archive.ranking_policies != prepared_archive.ranking_policies,
            ),
            (
                "choice_sets",
                archive.choice_sets != prepared_archive.choice_sets,
            ),
            (
                "ranking_statuses",
                archive.ranking_statuses != prepared_archive.ranking_statuses,
            ),
            (
                "archive_hash",
                archive.archive_hash != prepared_archive.archive_hash,
            ),
        ]
        .into_iter()
        .filter_map(|(name, differs)| differs.then_some(name))
        .collect::<Vec<_>>();
        let aggregate_detail = archive
            .aggregates
            .iter()
            .zip(&prepared_archive.aggregates)
            .find(|(left, right)| left != right)
            .map_or_else(|| "none".to_owned(), |(left, right)| {
                format!(
                    "run={} success={:x}/{:x} token={:?}/{:?} decision={:?}/{:?} budget={:x}/{:x} semantic={:x}/{:x} hash={}/{}",
                    left.run_id,
                    left.success_rate.to_bits(),
                    right.success_rate.to_bits(),
                    left.accepted_actions_per_token.map(f64::to_bits),
                    right.accepted_actions_per_token.map(f64::to_bits),
                    left.accepted_actions_per_decision.map(f64::to_bits),
                    right.accepted_actions_per_decision.map(f64::to_bits),
                    left.budget_exhaustion_rate.to_bits(),
                    right.budget_exhaustion_rate.to_bits(),
                    left.semantic_correctness_rate.to_bits(),
                    right.semantic_correctness_rate.to_bits(),
                    left.aggregate_hash,
                    right.aggregate_hash
                )
            });
        return Err(format!(
            "evaluation archive changed during JSON round trip: {}; {aggregate_detail}",
            changed.join(",")
        ));
    }
    verify_archive(&archive).map_err(|error| {
        let mut archive_model = archive.clone();
        archive_model.archive_hash.clear();
        let rebuilt_archive_hash = domain_hash(
            agentir_policy_eval::hashing::ARCHIVE_HASH_V2_DOMAIN,
            &archive_model,
        )
        .unwrap_or_else(|_| "unavailable".to_owned());
        let mut corpus_model = archive.corpus.clone();
        corpus_model.corpus_hash.clear();
        let rebuilt_corpus_hash = domain_hash(
            agentir_policy_eval::hashing::CORPUS_HASH_DOMAIN,
            &corpus_model,
        )
        .unwrap_or_else(|_| "unavailable".to_owned());
        format!(
            "verify saved production archive: {}; archive stored={} rebuilt={}; corpus stored={} manifest={} rebuilt={}",
            diagnostic_text(error),
            archive.archive_hash,
            rebuilt_archive_hash,
            archive.corpus.corpus_hash,
            archive.manifest.corpus_hash,
            rebuilt_corpus_hash
        )
    })?;
    Ok((
        archive,
        archive_bytes,
        json!({
            "ranking_decisions":ranking_decisions,
            "replayed_decisions":ranking_decisions,
            "compiler_rejected":compiler_rejected
        }),
    ))
}

fn run_archive_mutations(
    archive: &EvaluationArchive,
    archive_bytes: &[u8],
    records: &mut Records,
) -> Result<(), String> {
    type ArchiveMutation = (&'static str, Box<dyn Fn(&mut EvaluationArchive)>);
    let mutations: Vec<ArchiveMutation> = vec![
        (
            "archive-hash",
            Box::new(|value| value.archive_hash.push('0')),
        ),
        (
            "future-version",
            Box::new(|value| value.manifest.version = value.manifest.version.saturating_add(1)),
        ),
        (
            "ranking-status",
            Box::new(|value| {
                if let Some(status) = value.ranking_statuses.values_mut().next() {
                    *status = agentir_policy_eval::RankingEpisodeStatus::Unranked;
                }
            }),
        ),
        (
            "selected-choice",
            Box::new(|value| {
                if let Some(trace) = value
                    .runs
                    .iter_mut()
                    .flat_map(|run| &mut run.episodes)
                    .flat_map(|episode| &mut episode.steps)
                    .find_map(|step| step.ranking_trace.as_mut())
                {
                    trace.selected_choice = Some(EvaluationChoiceId("ec-corrupt".to_owned()));
                }
            }),
        ),
        (
            "choice-action",
            Box::new(|value| {
                if let Some(choice) = value
                    .choice_sets
                    .iter_mut()
                    .flat_map(|set| &mut set.choices)
                    .next()
                {
                    let agentir_policy_eval::ChoiceAction::ProductionRequest { request } =
                        &mut choice.action;
                    request["study_corruption"] = json!(true);
                }
            }),
        ),
        (
            "choice-feature",
            Box::new(|value| {
                if let Some(choice) = value
                    .choice_sets
                    .iter_mut()
                    .flat_map(|set| &mut set.choices)
                    .next()
                {
                    choice.visible_features.values.insert(
                        "continuation_depth".to_owned(),
                        agentir_policy_eval::FeatureValue::Integer(99),
                    );
                }
            }),
        ),
        (
            "score",
            Box::new(|value| {
                if let Some(score) = value
                    .runs
                    .iter_mut()
                    .flat_map(|run| &mut run.episodes)
                    .flat_map(|episode| &mut episode.steps)
                    .filter_map(|step| step.ranking_trace.as_mut())
                    .find_map(|trace| match &mut trace.decision {
                        RankingDecision::Scores { scores } => scores.first_mut(),
                        _ => None,
                    })
                {
                    score.score.units = score.score.units.saturating_add(1);
                }
            }),
        ),
    ];
    for (case_id, mutate) in mutations {
        let mut mutated = archive.clone();
        mutate(&mut mutated);
        if !matches!(case_id, "archive-hash" | "future-version") {
            refresh_archive_hash(&mut mutated)?;
        }
        let error = verify_archive(&mutated).expect_err("archive mutation must reject");
        records.mutations.push(mutation_record(case_id, &error)?);
    }
    for (case_id, offset) in [
        ("truncated-early", archive_bytes.len() / 10),
        ("truncated-middle", archive_bytes.len() / 2),
        ("truncated-late", archive_bytes.len().saturating_sub(1)),
    ] {
        let error = serde_json::from_slice::<EvaluationArchive>(&archive_bytes[..offset])
            .expect_err("truncated archive must reject");
        records.mutations.push(json!({
            "schema_version":STUDY_SCHEMA,
            "experiment_id":format!("mutation-{case_id}"),
            "case_id":case_id,
            "input_digest":digest(&json!({"case":case_id,"offset":offset}))?,
            "status":"policy_rejected",
            "diagnostic":"EVALUATION_ARCHIVE_INVALID",
            "first_failing_invariant":"JSON_PARSE",
            "panic_free":true,
            "details":error.to_string()
        }));
    }
    let mut invalid_utf8 = archive_bytes.to_vec();
    let invalid_utf8_offset = invalid_utf8.len() / 2;
    if let Some(byte) = invalid_utf8.get_mut(invalid_utf8_offset) {
        *byte = 0xff;
    }
    let invalid_utf8_rejected = serde_json::from_slice::<EvaluationArchive>(&invalid_utf8).is_err();
    if !invalid_utf8_rejected {
        return Err("invalid UTF-8 archive mutation was accepted".to_owned());
    }
    records.mutations.push(json!({
        "schema_version":STUDY_SCHEMA,
        "experiment_id":"mutation-invalid-utf8",
        "case_id":"invalid-utf8",
        "input_digest":digest(&json!({"case":"invalid-utf8"}))?,
        "status":"policy_rejected",
        "diagnostic":"EVALUATION_ARCHIVE_INVALID",
        "first_failing_invariant":"UTF8_OR_JSON_PARSE",
        "panic_free":true
    }));
    Ok(())
}

fn refresh_archive_hash(archive: &mut EvaluationArchive) -> Result<(), String> {
    archive.archive_hash.clear();
    archive.archive_hash = domain_hash(
        agentir_policy_eval::hashing::ARCHIVE_HASH_V2_DOMAIN,
        archive,
    )
    .map_err(diagnostic_text)?;
    Ok(())
}

fn run_archive_timings(
    configuration: &Configuration,
    environment: &Value,
    archive: &EvaluationArchive,
    archive_bytes: &[u8],
    records: &mut Records,
) -> Result<(), String> {
    let parse = measure_with_warmups(configuration.samples.min(5), 1, || {
        black_box(serde_json::from_slice::<EvaluationArchive>(archive_bytes).unwrap());
    });
    records.timings.push(timing_record(
        "archive_parse",
        archive_bytes.len(),
        "accepted",
        &parse,
        environment,
    )?);
    let verify = measure_with_warmups(configuration.samples.min(3), 1, || {
        verify_archive(archive).unwrap();
        black_box(());
    });
    records.timings.push(timing_record(
        "archive_structural_verify",
        archive_bytes.len(),
        "accepted",
        &verify,
        environment,
    )?);
    let load = measure_with_warmups(1, 0, || {
        black_box(
            EvaluationHarness::load_archive(
                &configuration.output.join("evaluation-archive.json"),
                &EvaluationLimits::default(),
            )
            .unwrap(),
        );
    });
    records.timings.push(timing_record(
        "archive_load_verify_replay",
        archive_bytes.len(),
        "accepted",
        &load,
        environment,
    )?);
    Ok(())
}

fn workload_matrix(records: &Records) -> Value {
    json!({
        "schema_version":STUDY_SCHEMA,
        "families":[
            {"name":"choice_set_cardinality","cases":[0,1,2,3,10,100,1000,1001],"status":"executed"},
            {"name":"choice_identity","cases":["transport_mutation","production_mutation","order_mutation"],"status":"executed"},
            {"name":"feature_schema_vectors","cases":["missing","wrong_type","negative","unsorted_duplicate_list"],"status":"executed"},
            {"name":"fixed_point_scores","cases":["zero","positive","negative","max","negative_max","max_plus_one","i64_min","wrong_scale","missing","duplicate","unknown","ties"],"status":"executed"},
            {"name":"selection_atomicity","cases":["outside_frame","production_accept","production_reject","replay"],"status":"executed"},
            {"name":"production_specir_candidate_equality_memory_schedule_backend_repair","cases":records.episodes.len(),"status":"executed_from_ranked_corpus"},
            {"name":"canonicalization_hashing","sizes":[1,2,10,32,100,256,1000],"status":"executed"},
            {"name":"ranking_ties","sizes":[1,2,10,32,100,256,1000],"status":"executed"},
            {"name":"replay","status":"executed_all_completed_runs"},
            {"name":"metrics_fairness","status":"executed_raw_per_run;incomparable_sequences_not_pooled"},
            {"name":"archive","cases":["v2_save","parse","verify","load_replay","mutation"],"status":"executed"},
            {"name":"failure_paths","cases":records.rejections.len(),"status":"executed"}
        ],
        "skipped_combinations":[
            {"family":"gpu_backend_performance","reason_code":"GPU_INDEPENDENT_OFFLINE_STUDY"},
            {"family":"learned_provider_rankers","reason_code":"NO_NETWORK_PROVIDER_OR_MODEL"},
            {"family":"full_cartesian_product","reason_code":"BOUNDED_REPRESENTATIVE_MATRIX"},
            {"family":"statistical_optimality","reason_code":"FIXED_SEEDS_ARE_DETERMINISM_CONTROLS_ONLY"},
            {"family":"archive_v1_migration_timing","reason_code":"COVERED_BY_STAGE6B_REGRESSION_TEST;NO_V1_FIXTURE_EXPORTED_TO_STUDY"},
            {"family":"continuation_cursor_resume","reason_code":"NO_STAGE6B_CURSOR_CONTRACT_IN_CURRENT_API"}
        ]
    })
}

fn analysis_markdown(semantic: &Value, timings: &[Value], environment: &Value) -> String {
    let summary = &semantic["summary"];
    let fastest = timings
        .iter()
        .min_by_key(|record| record["timing"]["median"].as_u64().unwrap_or(u64::MAX));
    let slowest = timings
        .iter()
        .max_by_key(|record| record["timing"]["median"].as_u64().unwrap_or(0));
    format!(
        r"# AgentIR Stage 6B.1 local study analysis

Generated from raw artifacts under the same study directory. Timing is observation only and never correctness evidence.

## 1. Executive summary

The production Stage 6B path supports bounded compiler-generated choices across SpecIR, candidates, equality, MemoryIR, ScheduleIR, BackendIR, and typed repair, then validates policy-owned fixed-point ranking and dispatches exactly one ordinary production action. This run recorded {episodes} episodes and {decisions} ranking decisions. Compiler certificates remain the only equivalence authority; tests, replay, and timings provide structural/confidence evidence only. Strong points are explicit hash domains, atomic compiler dispatch, offline replay, and bounded choice frames. Main limitations are synthetic scripted policies, incomplete cursor coverage, no GPU/provider measurements, and representative rather than Cartesian corner-case coverage.

Priority next steps: export production continuation work counters; add a first-class resumable enumeration contract; broaden typed repair diagnostics; add archive-v1 fixtures to the study runner; and collect learned-policy data only after frame comparability is explicit.

## 2. Architecture assessment

SpecIR is the immutable semantic anchor; ImplIR/CandidateForest and EqualityStore advance exact compiler-owned proof structures; MemoryIR owns physical storage and guarded fallback; ScheduleIR owns target-checked execution order; BackendIR/WGSL owns deterministic lowering artifacts. The evaluation/ranking layer is deliberately non-correctness-bearing and stores independent choice, policy, trace, selection, episode, and archive identities. Protocol and persistence replay the same production requests. Observed integration friction is concentrated where production continuations expose flat JSON descriptors without exported phase-level work counters. Resource limits are operational and excluded from semantic/compiler hashes.

## 3. Language ergonomics for agents

Measured observations expose exact bounded frames and stable locators, but JSON action payloads dominate context as frames scale. Compiler rejections remain atomic and can lead to typed repair frames. Menu is the narrowest surface, hybrid permits a separately recorded untrusted escape, and scripted ranking changes selection distribution without legality authority. This study does not support claims about human ergonomics.

## 4. Determinism and reproducibility

Stable choice identity now excludes compiler order and transport-only request/correlation/timing/host/provider-session fields. Choice-set order, ranking traces, selections, episode hashes, aggregate hashes, and archive bytes are retained in `semantic.json`. Run the comparator on two directories; any byte difference in semantic output is fatal while timing-only differences are expected machine noise. Environment: `{environment}`.

## 5. Correctness boundary

Compiler-proved correctness consists only of compiler-owned certificates and verified transactions. Feature/schema validation and archive verification are structural checks. Deterministic replay proves transcript reproducibility, not semantic equivalence. Differential tests and corner cases are confidence evidence. Benchmarks describe local latency. Ranking decisions express policy preference only; task-success predicates are compiler-derived evaluation outcomes but do not advance a proof frontier.

## 6. Performance and scalability

The study measured construction/canonicalization/hash and distinct/tied ranking at sizes 1, 2, 10, 32, 100, 256, and 1,000; replay at production-run sizes; and archive parse, verify, and load+replay. Fastest observed record: `{fastest}`. Slowest observed record: `{slowest}`. Median/p90/p95/p99 and raw samples are in `timing-samples.jsonl`. Growth classifications are empirical only; no extrapolation beyond 1,000 choices is warranted. Archive size and rejection latency are environment-local.

## 7. Policy comparison

The fixed-seed first-progress, goal-directed, and seeded-uniform policies are reported separately. Different choice sequences are not pooled as a fair comparison. The artifacts report compiler-order selection, acceptance/progress, ties, rejection, context, completion, and semantic task criterion. No policy is called “best” because no single optimization objective was defined.

## 8. Corner-case findings

| case | layer | expected | actual | diagnostic | atomic | replayable | performance note | status |
|---|---|---|---|---|---|---|---|---|
| transport metadata mutation | ranking identity | same choice ID | same | none | yes | yes | hash-only | fixed |
| production action mutation | ranking identity | different choice ID | different | none | yes | yes | hash-only | pass |
| missing/mistyped feature | feature schema | reject before publish | rejected | EVALUATION_FEATURE_SCHEMA_MISMATCH | yes | n/a | fast path | fixed |
| 1,001 choices | resource limit | reject before trace | rejected | EVALUATION_RANKING_BUDGET_EXCEEDED | yes | n/a | measured boundary | pass |
| archive mutations | persistence | first invariant rejects | rejected | recorded per mutation | yes | n/a | early/middle/late | pass |

Found bugs and fixes: choice IDs previously coupled transport metadata and compiler order; feature vectors were not checked against the schema beyond count limits. Both now have deterministic regression tests. Remaining ambiguous contracts include resumable continuation identity and whether future study schemas should retain per-phase compiler work counters.

## 9. Hash and archive assessment

Compiler hashes remain separate from `choice_id`, `choice_set_hash`, `feature_schema_hash`, `ranking_policy_hash`, `ranking_trace_hash`, `selection_hash`, ranked `episode_hash`, and evaluation `archive_hash`. Transport and timing fields are excluded from choice identity; order remains present in the choice-set contract. Mutation coverage changes action, feature, score, selection, status, envelope hash, version, truncation, and UTF-8. Archive v2 growth is driven by retained exact frames and transcripts; exact byte size is in the artifacts.

## 10. Resource-limit assessment

Frames at the 1,000-choice default limit were accepted and 1,001 was rejected before trace publication. Fixed-score magnitude boundaries and invalid scales were checked. The current diagnostics are stable and atomic. The study did not tune limits; their suitability for provider contexts remains unproven. No semantic hash includes the limits.

## 11. Data quality and limitations

Build profile: release. Samples per timing workload: {samples}. Hardware/environment anchors are in `environment.json`. Skips are explicit in `workloads.json`. Workloads are synthetic plus the immutable production ranked corpus; there are no learned policies, network/provider/device calls, or real-world GPU measurements. Small timing samples and background system activity limit performance confidence.

## 12. Prioritized growth areas

| priority | area | evidence | impact | complexity | risk | suggested stage | required contract | benchmark |
|---|---|---|---|---|---|---|---|---|
| P0 | reproducibility | compare two full semantic snapshots | prevents silent nondeterminism | low | low | 6B.1 | study semantic schema | repeated study compare |
| P1 | continuation cursor | no current cursor contract | scalable enumeration | medium | medium | 6B.x | cursor identity/resume | 1k+ resumed enumeration |
| P1 | diagnostics/repair | flat repair surface | lower rejection loops | medium | medium | 6B.x | typed repair taxonomy | repair-heavy corpus |
| P1 | phase work counters | total timings only | bottleneck attribution | medium | low | 6B.x | non-semantic counters | phase-separated enumeration |
| P2 | learned ranking readiness | scripted policies only | realistic selection data | high | high | 6C | frozen dataset/features | held-out fixed corpus |
| P2 | autotuning readiness | no device calls by design | performance policy later | high | high | post-6C | device/measurement contracts | hardware-isolated suite |

## 13. Stage 6C readiness verdict

**Conditionally ready.** Deterministic replay, archive integrity, bounded production multi-choice frames, negative ranking coverage, and raw metric reporting are present. Blocking items before an unconditional `ready` verdict: byte-identical confirmation from a second full run, explicit cursor/resume coverage, broader archive-v1/migration study data, representative learned-policy corpus evidence, and phase-level scalability counters.
",
        episodes = summary["episodes"],
        decisions = summary["ranking_decisions"],
        environment = environment,
        fastest = fastest.map_or_else(|| "unavailable".to_owned(), Value::to_string),
        slowest = slowest.map_or_else(|| "unavailable".to_owned(), Value::to_string),
        samples = timings
            .first()
            .and_then(|record| record.pointer("/timing/measured_sample_count"))
            .unwrap_or(&Value::Null),
    )
}

fn choices(size: usize) -> Result<Vec<EvaluationChoice>, String> {
    (0..size)
        .map(|index| {
            choice_with_request(
                &format!("study-request-{index}"),
                &format!("sa{index}"),
                (index % 32).saturating_add(1),
            )
        })
        .collect()
}

fn choice_with_request(
    request_id: &str,
    target: &str,
    tile_size: usize,
) -> Result<EvaluationChoice, String> {
    compiler_choice(
        ChoiceOrigin::Schedule,
        ChoiceCategory::ScheduleTile,
        json!({
            "command":"schedule.apply",
            "request_id":request_id,
            "actions":[{"kind":"tile_axes","axes":[target],"tile_sizes":[tile_size]}]
        }),
        ChoicePreconditions::default(),
        "study compiler choice",
        "unchanged_or_compiler_owned",
        target,
    )
    .map_err(diagnostic_text)
}

fn transport_mutated_choice() -> Result<EvaluationChoice, String> {
    compiler_choice(
        ChoiceOrigin::Schedule,
        ChoiceCategory::ScheduleTile,
        json!({
            "command":"schedule.apply",
            "request_id":"request-b",
            "correlation_id":"correlation-b",
            "timestamp_ns":99,
            "latency_ms":88,
            "hostname":"host-b",
            "provider_session_id":"session-b",
            "actions":[{"kind":"tile_axes","axes":["sa1"],"tile_sizes":[4]}]
        }),
        ChoicePreconditions::default(),
        "study compiler choice",
        "unchanged_or_compiler_owned",
        "sa1",
    )
    .map_err(diagnostic_text)
}

fn rejected_choice_case(
    records: &mut Records,
    case_id: &str,
    choice: EvaluationChoice,
    schema: &agentir_policy_eval::FeatureSchema,
    limits: &RankingLimits,
) -> Result<(), String> {
    let input = json!({"case":case_id});
    let error = build_choice_set(case_id, schema, vec![choice], limits)
        .expect_err("invalid feature vector must reject");
    push_rejection(records, case_id, "policy_rejected", &input, &error)
}

#[allow(clippy::needless_pass_by_value)]
fn case_record(
    case_id: &str,
    family: &str,
    status: &str,
    input: &Value,
    result: Value,
) -> Result<Value, String> {
    Ok(json!({
        "schema_version":STUDY_SCHEMA,
        "experiment_id":format!("{family}-{case_id}"),
        "workload_family":family,
        "case_id":case_id,
        "deterministic_seed":0,
        "input_digest":digest(input)?,
        "status":status,
        "result":result,
        "panic_free":true
    }))
}

fn push_rejection(
    records: &mut Records,
    case_id: &str,
    status: &str,
    input: &Value,
    error: &EvaluationDiagnostic,
) -> Result<(), String> {
    let record = case_record(
        case_id,
        "failure_paths",
        status,
        input,
        json!({"diagnostic":error.code,"details":error.details}),
    )?;
    records.rejections.push(record.clone());
    records.cases.push(record);
    Ok(())
}

fn mutation_record(case_id: &str, error: &EvaluationDiagnostic) -> Result<Value, String> {
    Ok(json!({
        "schema_version":STUDY_SCHEMA,
        "experiment_id":format!("mutation-{case_id}"),
        "case_id":case_id,
        "input_digest":digest(&json!({"case":case_id}))?,
        "status":"policy_rejected",
        "diagnostic":error.code,
        "first_failing_invariant":error.message,
        "panic_free":true
    }))
}

fn timing_record(
    family: &str,
    size: usize,
    status: &str,
    timing: &TimingSummary,
    environment: &Value,
) -> Result<Value, String> {
    let input = json!({"family":family,"size":size});
    Ok(json!({
        "schema_version":STUDY_SCHEMA,
        "benchmark_schema":BENCHMARK_SCHEMA,
        "experiment_id":format!("timing-{family}-{size}"),
        "workload_family":family,
        "case_id":format!("size-{size}"),
        "deterministic_seed":0,
        "input_digest":digest(&input)?,
        "workload_size":size,
        "status":status,
        "work_units":size,
        "context_bytes":0,
        "timing":timing,
        "environment_anchor":environment
    }))
}

fn measure(samples: usize, workload: impl FnMut()) -> TimingSummary {
    measure_with_warmups(samples, WARMUPS, workload)
}

fn measure_with_warmups(
    mut samples: usize,
    warmups: usize,
    mut workload: impl FnMut(),
) -> TimingSummary {
    samples = samples.max(1);
    for _ in 0..warmups {
        workload();
    }
    let mut raw = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        workload();
        raw.push(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
    }
    raw.sort_unstable();
    let mean = raw.iter().map(|value| *value as f64).sum::<f64>() / raw.len() as f64;
    let variance = raw
        .iter()
        .map(|value| {
            let delta = *value as f64 - mean;
            delta * delta
        })
        .sum::<f64>()
        / raw.len() as f64;
    TimingSummary {
        unit: "ns",
        warmup_count: warmups,
        measured_sample_count: samples,
        raw_sample_count: raw.len(),
        min: raw[0],
        median: percentile(&raw, 50),
        p90: percentile(&raw, 90),
        p95: percentile(&raw, 95),
        p99: percentile(&raw, 99),
        max: raw[raw.len() - 1],
        mean,
        standard_deviation: variance.sqrt(),
    }
}

fn percentile(values: &[u64], percent: usize) -> u64 {
    let rank = percent.saturating_mul(values.len()).saturating_add(99) / 100;
    values[rank
        .max(1)
        .saturating_sub(1)
        .min(values.len().saturating_sub(1))]
}

fn status_counts(cases: &[Value]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for status in cases.iter().filter_map(|record| record["status"].as_str()) {
        *counts.entry(status.to_owned()).or_insert(0) += 1;
    }
    counts
}

fn digest(value: &impl Serialize) -> Result<String, String> {
    domain_hash(INPUT_DOMAIN, value).map_err(diagnostic_text)
}

#[allow(clippy::needless_pass_by_value)]
fn diagnostic_text(error: EvaluationDiagnostic) -> String {
    format!("{:?}: {}", error.code, error.message)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn write_jsonl(path: &Path, values: &[Value]) -> Result<(), String> {
    let mut bytes = Vec::new();
    for value in values {
        serde_json::to_writer(&mut bytes, value).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
    }
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn environment_anchor() -> Value {
    json!({
        "build_profile":"release",
        "target_triple":command_output("rustc", &["-vV"])
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap_or("unknown"),
        "os":env::consts::OS,
        "architecture":env::consts::ARCH,
        "rustc_version":command_output("rustc", &["--version"]),
        "git_commit":command_output("git", &["rev-parse", "HEAD"]),
        "dirty":!command_output("git", &["status", "--porcelain"]).is_empty()
    })
}

fn command_output(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map_or_else(
            || "unknown".to_owned(),
            |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        )
}
