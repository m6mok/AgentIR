//! Dependency-light statistical Stage 6A/6B baseline using benchmark schema v2.

use agentir_policy_eval::{
    ChoiceCategory, ChoiceOrigin, ChoicePreconditions, EvaluationHarness, EvaluationTaskId,
    RankingLimits, build_choice_set, compiler_choice, feature_schema_v1, rank_choices,
    scripted_ranker, scripted_ranking_decision,
};
use serde_json::{Value, json};
use std::{hint::black_box, time::Instant};

const WARMUPS: usize = 2;
const SAMPLES: usize = 9;

fn percentile(samples: &[u64], percentile: usize) -> u64 {
    let rank = percentile.saturating_mul(samples.len()).saturating_add(99) / 100;
    samples[rank
        .max(1)
        .saturating_sub(1)
        .min(samples.len().saturating_sub(1))]
}

fn measure(mut workload: impl FnMut()) -> Value {
    for _ in 0..WARMUPS {
        workload();
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        workload();
        samples.push(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
    }
    samples.sort_unstable();
    json!({
        "unit": "ns",
        "samples": SAMPLES,
        "min": samples[0],
        "median": percentile(&samples, 50),
        "p95": percentile(&samples, 95),
        "max": samples[samples.len() - 1]
    })
}

fn main() {
    let policies = [
        "free_reference_v1",
        "menu_first_valid_v1",
        "menu_goal_directed_v1",
        "hybrid_menu_preferred_v1",
        "hybrid_bounded_escape_v1",
    ];
    let tasks = [EvaluationTaskId("saxpy-end-to-end-large".to_owned())];
    let mut records: Vec<Value> = policies
        .iter()
        .map(|policy| {
            let timing = measure(|| {
                let mut harness = EvaluationHarness::new().expect("built-in corpus");
                let run = harness
                    .run_scripted(policy, &tasks, &[0])
                    .expect("scripted baseline");
                harness.replay_run(&run).expect("deterministic replay");
                black_box(harness.aggregate(&run).expect("aggregate"));
            });
            json!({
                "workload": "scripted_episode_apply_replay_aggregate",
                "policy": policy,
                "task_count": 1,
                "step_count": 5,
                "timing": timing
            })
        })
        .collect();
    let schema = feature_schema_v1().expect("feature schema");
    let limits = RankingLimits::default();
    for size in [10_usize, 100, 1_000] {
        let choices = (0..size)
            .map(|index| {
                compiler_choice(
                    ChoiceOrigin::Schedule,
                    ChoiceCategory::ScheduleTile,
                    json!({
                        "command":"schedule.apply",
                        "request_id":format!("benchmark-{index}"),
                        "actions":[{"kind":"tile_axes","axes":["sa1"],"tile_sizes":[index + 1]}]
                    }),
                    ChoicePreconditions::default(),
                    "benchmark compiler choice",
                    "unchanged_or_compiler_owned",
                    "sa1",
                )
                .expect("benchmark choice")
            })
            .collect::<Vec<_>>();
        let set = build_choice_set("benchmark-observation", &schema, choices, &limits)
            .expect("choice set");
        let policy =
            scripted_ranker("seeded_uniform_choice_v1", &schema, 23).expect("scripted ranker");
        let timing = measure(|| {
            let decision = scripted_ranking_decision(&policy, &set, &limits)
                .expect("scripted ranking decision");
            black_box(rank_choices(&set, &policy, decision, &limits).expect("ranking trace"));
        });
        records.push(json!({
            "workload":"choice_hash_feature_scripted_rank_tie_select",
            "choice_count":size,
            "timing":timing
        }));
    }
    let corpus = agentir_policy_eval::builtin_corpus().expect("built-in corpus");
    let sizes = json!({
        "corpus_tasks": corpus.tasks.len(),
        "corpus_bytes": serde_json::to_vec(&corpus).expect("corpus bytes").len(),
        "feature_schema_bytes": serde_json::to_vec(&schema).expect("schema bytes").len(),
        "canonical_sizes_are_timings": false
    });
    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema": "agentir_policy_evaluation_baseline_v1",
            "benchmark_schema": 2,
            "warmups": WARMUPS,
            "timings_are_correctness": false,
            "records": records,
            "sizes": sizes
        }))
        .expect("baseline JSON")
    );
}
