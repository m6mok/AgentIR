use serde_json::json;
use std::{env, fs, path::Path};

const SEMANTIC_FILES: &[&str] = &[
    "campaign-plan.json",
    "campaign-session.json",
    "campaign-checkpoint.json",
    "campaign-result.json",
    "scenarios.json",
    "replay.json",
    "mutations.json",
    "archive-v8.json",
    "metrics.json",
];

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("usage: stage7e_compare RUN_1 RUN_2".to_owned());
    }
    let left = Path::new(&args[0]);
    let right = Path::new(&args[1]);
    for name in SEMANTIC_FILES {
        if fs::read(left.join(name)).map_err(|error| error.to_string())?
            != fs::read(right.join(name)).map_err(|error| error.to_string())?
        {
            return Err(format!("Stage 7E semantic file differs: {name}"));
        }
    }
    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema_version":"agentir.stage7e.compare.v1",
            "semantic_files":SEMANTIC_FILES.len(),
            "byte_identical":true,
            "operational_wall_clock_compared":false,
        }))
        .expect("comparison")
    );
    Ok(())
}
