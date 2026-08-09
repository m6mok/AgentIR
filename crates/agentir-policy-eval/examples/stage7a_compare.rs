use serde_json::{Value, json};
use std::{env, fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments.len() != 2 {
        return Err("usage: stage7a_compare RUN_1 RUN_2".into());
    }
    let first = Path::new(&arguments[0]);
    let second = Path::new(&arguments[1]);
    let compare = |name: &str| -> Result<bool, std::io::Error> {
        Ok(fs::read(first.join(name))? == fs::read(second.join(name))?)
    };
    let semantic_equal = compare("semantic.json")?;
    let results_equal = compare("search-results.jsonl")?;
    let checkpoints_equal = compare("checkpoints.jsonl")?;
    let archive_equal = compare("evaluation-archive.json")?;
    let mutations_equal = compare("mutation-results.jsonl")?;
    let status =
        if semantic_equal && results_equal && checkpoints_equal && archive_equal && mutations_equal
        {
            "expected_timing_noise_only"
        } else {
            "semantic_mismatch"
        };
    let result: Value = json!({
        "schema_version":"agentir.stage7a.reproducibility.v1",
        "semantic_byte_identical":semantic_equal,
        "search_results_byte_identical":results_equal,
        "checkpoints_byte_identical":checkpoints_equal,
        "archive_byte_identical":archive_equal,
        "mutation_classification_identical":mutations_equal,
        "timing_compared":false,
        "status":status
    });
    let bytes = serde_json::to_vec_pretty(&result)?;
    if let Some(parent) = first.parent()
        && second.parent() == Some(parent)
    {
        fs::write(parent.join("reproducibility.json"), &bytes)?;
    }
    println!("{}", serde_json::to_string(&result)?);
    if status == "expected_timing_noise_only" {
        Ok(())
    } else {
        Err("Stage 7A semantic, result, checkpoint, archive, or mutation mismatch".into())
    }
}
