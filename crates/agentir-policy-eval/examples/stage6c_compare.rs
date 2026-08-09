use serde_json::{Value, json};
use std::{env, fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments.len() != 2 {
        return Err("usage: stage6c_compare RUN_1 RUN_2".into());
    }
    let first = Path::new(&arguments[0]);
    let second = Path::new(&arguments[1]);
    let first_semantic = fs::read(first.join("semantic.json"))?;
    let second_semantic = fs::read(second.join("semantic.json"))?;
    let first_archive = fs::read(first.join("evaluation-archive.json"))?;
    let second_archive = fs::read(second.join("evaluation-archive.json"))?;
    let semantic_equal = first_semantic == second_semantic;
    let archive_equal = first_archive == second_archive;
    let status = if semantic_equal && archive_equal {
        "expected_timing_noise_only"
    } else {
        "semantic_mismatch"
    };
    let result: Value = json!({
        "schema_version":"agentir.stage6c.reproducibility.v1",
        "semantic_byte_identical":semantic_equal,
        "archive_byte_identical":archive_equal,
        "timing_compared":false,
        "status":status
    });
    println!("{}", serde_json::to_string(&result)?);
    if semantic_equal && archive_equal {
        Ok(())
    } else {
        Err("Stage 6C semantic or archive reproduction mismatch".into())
    }
}
