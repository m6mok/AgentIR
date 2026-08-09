//! Compares two Stage 6B study directories, failing on semantic differences.

use serde_json::{Value, json};
use std::{env, fs, path::PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("stage6b study comparison failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let directories = env::args().skip(1).map(PathBuf::from).collect::<Vec<_>>();
    if directories.len() != 2 {
        return Err("usage: stage6b_compare FIRST_STUDY_DIR SECOND_STUDY_DIR".to_owned());
    }
    let first_semantic = fs::read(directories[0].join("semantic.json"))
        .map_err(|error| format!("first semantic snapshot: {error}"))?;
    let second_semantic = fs::read(directories[1].join("semantic.json"))
        .map_err(|error| format!("second semantic snapshot: {error}"))?;
    let first_environment = read_json(&directories[0].join("environment.json"))?;
    let second_environment = read_json(&directories[1].join("environment.json"))?;
    let environment_match = first_environment == second_environment;
    let semantic_match = first_semantic == second_semantic;
    let classification = if !semantic_match {
        "semantic_mismatch"
    } else if !environment_match {
        "environment_mismatch"
    } else {
        "expected_timing_noise_only"
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema_version":"agentir.stage6b.study.comparison.v1",
            "semantic_byte_identical":semantic_match,
            "environment_identical":environment_match,
            "classification":classification,
            "timing_samples_compared":false,
            "timing_difference_policy":"expected machine noise"
        }))
        .map_err(|error| error.to_string())?
    );
    if semantic_match {
        Ok(())
    } else {
        Err("semantic snapshots differ byte-for-byte".to_owned())
    }
}

fn read_json(path: &PathBuf) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}
