use serde_json::Value;
use std::{collections::BTreeSet, fs, path::PathBuf};

fn registry() -> Value {
    serde_json::from_str(include_str!("../../../docs/contract-registry.json")).unwrap()
}

fn unique_strings(registry: &Value, pointer: &str) {
    let values = registry.pointer(pointer).unwrap().as_array().unwrap();
    let mut unique = BTreeSet::new();
    for value in values {
        assert!(
            unique.insert(value.as_str().unwrap()),
            "duplicate registry entry: {value}"
        );
    }
}

#[test]
fn registry_has_unique_hash_diagnostic_and_id_domains() {
    let registry = registry();
    unique_strings(&registry, "/hash_domains");
    unique_strings(&registry, "/diagnostic_codes");
    unique_strings(&registry, "/id_prefixes");
    unique_strings(&registry, "/continuation_cursor_versions");
    unique_strings(&registry, "/feature_codecs");
    unique_strings(&registry, "/model_formats");
}

#[test]
fn registry_documents_current_archives_and_every_migration_edge() {
    let registry = registry();
    let families = registry["archive_families"].as_array().unwrap();
    assert!(
        families
            .iter()
            .any(|family| family["name"] == "workspace" && family["current"] == 10)
    );
    assert!(
        families
            .iter()
            .any(|family| family["name"] == "evaluation" && family["current"] == 8)
    );
    let edges = registry["migration_edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    for version in 1..10 {
        assert!(edges.contains(format!("workspace:{version}->{}", version + 1).as_str()));
    }
    assert!(edges.contains("evaluation:1->2"));
    assert!(edges.contains("evaluation:2->3"));
    assert!(edges.contains("evaluation:3->4"));
    assert!(edges.contains("evaluation:4->5"));
    assert!(edges.contains("evaluation:5->6"));
    assert!(edges.contains("evaluation:6->7"));
    assert!(edges.contains("evaluation:7->8"));
}

#[test]
fn every_public_contract_document_exists_and_is_nonempty() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for document in registry()["public_contract_documents"].as_array().unwrap() {
        let path = root.join(document.as_str().unwrap());
        let contents = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "missing public contract document {}: {error}",
                path.display()
            )
        });
        assert!(
            !contents.trim().is_empty(),
            "empty contract document: {}",
            path.display()
        );
    }
}

#[test]
fn registered_evaluation_diagnostics_exist_in_the_stable_enum() {
    let source = include_str!("../src/model.rs");
    for code in registry()["diagnostic_codes"].as_array().unwrap() {
        let code = code.as_str().unwrap();
        assert!(
            source.contains(&format!("    {code},")),
            "undocumented diagnostic code: {code}"
        );
    }
}
