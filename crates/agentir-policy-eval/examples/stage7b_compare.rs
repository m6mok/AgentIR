use serde_json::json;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn files(path: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(path)
        .expect("study directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()) != Some("timing-observations.json")
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("usage: stage7b_compare RUN_1 RUN_2".to_owned());
    }
    let left = Path::new(&args[0]);
    let right = Path::new(&args[1]);
    let left_files = files(left);
    let right_files = files(right);
    let left_names = left_files
        .iter()
        .map(|path| path.file_name().unwrap().to_owned())
        .collect::<Vec<_>>();
    let right_names = right_files
        .iter()
        .map(|path| path.file_name().unwrap().to_owned())
        .collect::<Vec<_>>();
    if left_names != right_names {
        return Err("semantic study file sets differ".to_owned());
    }
    for name in &left_names {
        if fs::read(left.join(name)).map_err(|error| error.to_string())?
            != fs::read(right.join(name)).map_err(|error| error.to_string())?
        {
            return Err(format!(
                "semantic study file differs: {}",
                name.to_string_lossy()
            ));
        }
    }
    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema_version": "agentir.stage7b.compare.v1",
            "semantic_files": left_names.len(),
            "byte_identical": true,
            "timing_compared_as_observation_only": true
        }))
        .expect("comparison")
    );
    Ok(())
}
