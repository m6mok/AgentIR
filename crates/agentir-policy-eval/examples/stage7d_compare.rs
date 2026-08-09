use serde_json::json;
use std::{env, fs, path::Path};

const SEMANTIC_FILES: &[&str] = &[
    "plan.json",
    "scenarios.json",
    "prepared-slots.json",
    "journals.json",
    "reconciliations.json",
    "stage7c-result.json",
    "replay.json",
    "cohort.json",
    "measured-objective.json",
    "measured-recommendation.json",
    "mutations.json",
    "archive-v7.json",
    "metrics.json",
];

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("usage: stage7d_compare RUN_1 RUN_2".to_owned());
    }
    let left = Path::new(&args[0]);
    let right = Path::new(&args[1]);
    for name in SEMANTIC_FILES {
        let left_bytes = fs::read(left.join(name)).map_err(|error| error.to_string())?;
        let right_bytes = fs::read(right.join(name)).map_err(|error| error.to_string())?;
        if left_bytes != right_bytes {
            return Err(format!("Stage 7D semantic file differs: {name}"));
        }
    }
    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema_version":"agentir.stage7d.compare.v1",
            "semantic_files":SEMANTIC_FILES.len(),
            "byte_identical":true,
            "operational_wall_clock_compared":false,
        }))
        .expect("comparison")
    );
    Ok(())
}
