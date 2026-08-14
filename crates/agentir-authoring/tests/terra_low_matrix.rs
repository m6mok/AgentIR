use agentir_authoring::{
    GRAPH_SCHEMA, GraphOpcode, GraphOperand, GraphOperation, GraphProposal, parse_proposal,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn scalar(name: impl Into<String>) -> GraphOperand {
    GraphOperand::Scalar { name: name.into() }
}

fn tensor(name: impl Into<String>) -> GraphOperand {
    GraphOperand::Tensor { name: name.into() }
}

const fn local(operation: usize) -> GraphOperand {
    GraphOperand::Local { operation }
}

fn op(op: GraphOpcode, operands: Vec<GraphOperand>) -> GraphOperation {
    GraphOperation { op, operands }
}

fn proposal(operations: Vec<GraphOperation>) -> GraphProposal {
    GraphProposal {
        schema: GRAPH_SCHEMA.to_owned(),
        r#yield: operations.len() - 1,
        operations,
    }
}

fn short(variant: usize) -> GraphProposal {
    use GraphOpcode::{Add, Fma, Mul};
    let operations = match variant {
        1 => vec![
            op(Fma, vec![tensor("river"), scalar("cobalt"), tensor("dune")]),
            op(Mul, vec![tensor("mist"), local(0)]),
            op(Add, vec![local(1), tensor("stone")]),
            op(Mul, vec![local(0), local(2)]),
            op(Add, vec![local(3), local(1)]),
            op(Fma, vec![local(4), scalar("amber"), local(0)]),
            op(Mul, vec![local(5), tensor("river")]),
            op(Add, vec![tensor("dune"), local(6)]),
            op(Mul, vec![local(2), local(7)]),
            op(Fma, vec![scalar("cobalt"), local(8), local(5)]),
            op(Add, vec![local(9), local(3)]),
            op(Mul, vec![tensor("stone"), local(10)]),
        ],
        2 => vec![
            op(Mul, vec![scalar("north"), tensor("leaf")]),
            op(Fma, vec![tensor("rain"), scalar("south"), tensor("rock")]),
            op(Add, vec![local(1), local(0)]),
            op(Mul, vec![tensor("wind"), local(2)]),
            op(Add, vec![local(0), local(3)]),
            op(Fma, vec![local(4), scalar("north"), local(1)]),
            op(Mul, vec![local(5), local(2)]),
            op(Add, vec![tensor("leaf"), local(6)]),
            op(Fma, vec![scalar("south"), local(7), local(3)]),
            op(Add, vec![local(8), local(0)]),
            op(Mul, vec![local(9), tensor("rain")]),
            op(Add, vec![local(10), local(5)]),
        ],
        3 => vec![
            op(Fma, vec![scalar("prism"), tensor("flare"), tensor("mist")]),
            op(Mul, vec![local(0), tensor("shore")]),
            op(Add, vec![tensor("grove"), local(1)]),
            op(Add, vec![local(2), local(0)]),
            op(Mul, vec![scalar("ember"), local(3)]),
            op(Fma, vec![local(4), scalar("prism"), local(1)]),
            op(Add, vec![local(5), local(2)]),
            op(Mul, vec![tensor("flare"), local(6)]),
            op(Add, vec![local(7), local(0)]),
            op(Fma, vec![tensor("mist"), scalar("ember"), local(8)]),
            op(Mul, vec![local(9), local(3)]),
            op(Add, vec![local(10), local(5)]),
        ],
        4 => vec![
            op(Mul, vec![tensor("harbor"), scalar("quartz")]),
            op(Add, vec![local(0), tensor("cedar")]),
            op(Fma, vec![local(1), scalar("lime"), tensor("orbit")]),
            op(Mul, vec![local(2), local(0)]),
            op(Add, vec![tensor("pearl"), local(3)]),
            op(Mul, vec![local(1), local(4)]),
            op(Fma, vec![scalar("quartz"), local(5), local(2)]),
            op(Add, vec![local(6), local(0)]),
            op(Mul, vec![tensor("cedar"), local(7)]),
            op(Add, vec![local(8), local(4)]),
            op(Fma, vec![local(9), scalar("lime"), local(1)]),
            op(Add, vec![local(10), local(6)]),
        ],
        _ => unreachable!(),
    };
    proposal(operations)
}

fn recurrence(variant: usize) -> GraphProposal {
    use GraphOpcode::{Add, Fma, Mul};
    let mut operations = match variant {
        1 => vec![
            op(Mul, vec![scalar("a0"), tensor("x0")]),
            op(Fma, vec![tensor("x1"), scalar("a1"), tensor("x2")]),
            op(Add, vec![local(0), local(1)]),
            op(Mul, vec![tensor("x3"), local(2)]),
            op(Fma, vec![scalar("a2"), tensor("x4"), local(3)]),
            op(Add, vec![tensor("x5"), local(4)]),
            op(Mul, vec![local(2), local(5)]),
            op(Fma, vec![local(6), scalar("a3"), tensor("x6")]),
        ],
        2 => vec![
            op(Fma, vec![scalar("p0"), tensor("u0"), tensor("u1")]),
            op(Mul, vec![tensor("u2"), local(0)]),
            op(Add, vec![local(1), tensor("u3")]),
            op(Mul, vec![local(0), local(2)]),
            op(Fma, vec![tensor("u4"), scalar("p1"), local(3)]),
            op(Add, vec![tensor("u5"), local(4)]),
            op(Mul, vec![scalar("p2"), local(5)]),
            op(Add, vec![local(6), local(1)]),
        ],
        3 => vec![
            op(Mul, vec![tensor("v0"), scalar("k0")]),
            op(Fma, vec![scalar("k1"), tensor("v1"), tensor("v2")]),
            op(Add, vec![local(1), local(0)]),
            op(Mul, vec![local(2), tensor("v3")]),
            op(Fma, vec![local(3), scalar("k2"), tensor("v4")]),
            op(Add, vec![tensor("v5"), local(4)]),
            op(Mul, vec![local(5), local(2)]),
            op(Fma, vec![tensor("v6"), scalar("k3"), local(6)]),
        ],
        4 => vec![
            op(Fma, vec![tensor("q0"), scalar("z0"), tensor("q1")]),
            op(Add, vec![local(0), tensor("q2")]),
            op(Mul, vec![tensor("q3"), local(1)]),
            op(Fma, vec![local(2), scalar("z1"), local(0)]),
            op(Add, vec![tensor("q4"), local(3)]),
            op(Mul, vec![scalar("z2"), local(4)]),
            op(Add, vec![local(5), local(1)]),
            op(Fma, vec![tensor("q5"), scalar("z3"), local(6)]),
        ],
        _ => unreachable!(),
    };
    for n in 8..48 {
        operations.push(match variant {
            1 => match n % 4 {
                0 => op(Add, vec![local(n - 1), local(n - 8)]),
                1 => op(Mul, vec![tensor(format!("x{}", n % 8)), local(n - 1)]),
                2 => op(
                    Fma,
                    vec![
                        scalar(format!("a{}", (n / 4) % 4)),
                        local(n - 1),
                        local(n - 6),
                    ],
                ),
                _ => op(Add, vec![local(n - 1), tensor(format!("x{}", (n + 2) % 8))]),
            },
            2 => match n % 5 {
                0 => op(
                    Fma,
                    vec![
                        local(n - 1),
                        scalar(format!("p{}", (n / 5) % 5)),
                        local(n - 7),
                    ],
                ),
                1 => op(Add, vec![tensor(format!("u{}", (n + 4) % 9)), local(n - 1)]),
                2 => op(Mul, vec![local(n - 1), tensor(format!("u{}", n % 9))]),
                3 => op(Add, vec![local(n - 1), local(n - 8)]),
                _ => op(Mul, vec![scalar(format!("p{}", (n + 2) % 5)), local(n - 1)]),
            },
            3 => match n % 4 {
                0 => op(Add, vec![local(n - 1), local(n - 5)]),
                1 => op(
                    Fma,
                    vec![
                        scalar(format!("k{}", (n / 4) % 4)),
                        local(n - 1),
                        local(n - 8),
                    ],
                ),
                2 => op(Mul, vec![tensor(format!("v{}", (n + 1) % 8)), local(n - 1)]),
                _ => op(Add, vec![tensor(format!("v{}", (n + 3) % 8)), local(n - 1)]),
            },
            4 => match n % 5 {
                0 => op(Add, vec![local(n - 1), local(n - 8)]),
                1 => op(Mul, vec![local(n - 1), tensor(format!("q{}", n % 9))]),
                2 => op(
                    Fma,
                    vec![
                        local(n - 1),
                        scalar(format!("z{}", (n / 5) % 5)),
                        local(n - 6),
                    ],
                ),
                3 => op(Add, vec![tensor(format!("q{}", (n + 4) % 9)), local(n - 1)]),
                _ => op(Mul, vec![scalar(format!("z{}", (n + 1) % 5)), local(n - 1)]),
            },
            _ => unreachable!(),
        });
    }
    proposal(operations)
}

struct Design<'a> {
    seed: &'a str,
    first_scalar: &'a str,
    second_scalar: &'a str,
    bias: &'a str,
    mask: &'a str,
    source: &'a str,
    anchor: &'a str,
}

fn design(variant: usize) -> GraphProposal {
    use GraphOpcode::{Add, Fma, Mul};
    let names = match variant {
        1 => Design {
            seed: "origin",
            first_scalar: "c",
            second_scalar: "d",
            bias: "b",
            mask: "m",
            source: "r",
            anchor: "y",
        },
        2 => Design {
            seed: "start",
            first_scalar: "alpha",
            second_scalar: "beta",
            bias: "offset",
            mask: "gate",
            source: "feed",
            anchor: "skip",
        },
        3 => Design {
            seed: "seed",
            first_scalar: "s",
            second_scalar: "t",
            bias: "bias",
            mask: "mask",
            source: "src",
            anchor: "anchor",
        },
        4 => Design {
            seed: "root",
            first_scalar: "w",
            second_scalar: "e",
            bias: "pad",
            mask: "filter",
            source: "stream",
            anchor: "carry",
        },
        _ => unreachable!(),
    };
    let mut operations = Vec::with_capacity(96);
    for i in 0..16 {
        let base = i * 6;
        let state = if i == 0 {
            tensor(names.seed)
        } else {
            local(base - 1)
        };
        let (first, bias, mask, source, second) = match variant {
            1 => (
                i % 5,
                (2 * i + 1) % 4,
                (3 * i + 2) % 5,
                (5 * i + 3) % 7,
                (i + 2) % 4,
            ),
            2 => (
                (i + 1) % 5,
                (3 * i + 2) % 4,
                (2 * i + 4) % 5,
                (4 * i + 1) % 7,
                (i + 3) % 4,
            ),
            3 => (
                (2 * i + 3) % 5,
                (i + 2) % 4,
                (4 * i + 1) % 5,
                (3 * i + 5) % 7,
                (i + 1) % 4,
            ),
            4 => (
                (4 * i + 2) % 5,
                (3 * i + 1) % 4,
                (i + 3) % 5,
                (2 * i + 4) % 7,
                (3 * i + 2) % 4,
            ),
            _ => unreachable!(),
        };
        operations.extend([
            op(
                Fma,
                vec![
                    scalar(format!("{}{first}", names.first_scalar)),
                    state.clone(),
                    tensor(format!("{}{bias}", names.bias)),
                ],
            ),
            op(
                Mul,
                vec![tensor(format!("{}{mask}", names.mask)), local(base)],
            ),
            op(
                Add,
                vec![tensor(format!("{}{source}", names.source)), local(base)],
            ),
            op(Add, vec![local(base + 1), local(base + 2)]),
            op(
                Fma,
                vec![
                    local(base + 3),
                    scalar(format!("{}{second}", names.second_scalar)),
                    state,
                ],
            ),
            op(
                Add,
                vec![
                    local(base + 4),
                    if i < 4 {
                        tensor(format!("{}{i}", names.anchor))
                    } else {
                        local((i - 3) * 6 + 5)
                    },
                ],
            ),
        ]);
    }
    proposal(operations)
}

fn diagnostic_normalize(payload: &str, scalars: &[&str]) -> GraphProposal {
    let mut value: Value = serde_json::from_str(payload).expect("valid JSON");
    let root = value.as_object_mut().expect("proposal object");
    root.remove("scalars");
    root.remove("tensors");
    root.insert("schema".to_owned(), json!(GRAPH_SCHEMA));
    let scalar_names = scalars.iter().copied().collect::<BTreeSet<_>>();
    for operation in root["operations"].as_array_mut().expect("operations") {
        for operand in operation["operands"].as_array_mut().expect("operands") {
            if let Some(name) = operand.as_str().map(str::to_owned) {
                *operand = if scalar_names.contains(name.as_str()) {
                    json!({"kind":"scalar","name":name})
                } else {
                    json!({"kind":"tensor","name":name})
                };
            } else if let Some(object) = operand.as_object_mut()
                && object.get("kind") == Some(&json!("local"))
                && let Some(index) = object.remove("integer")
            {
                object.insert("operation".to_owned(), index);
            }
        }
    }
    parse_proposal(&serde_json::to_string(&value).expect("serialize normalized proposal"))
        .expect("diagnostic aliases normalize into the strict graph type")
}

struct Trial<'a> {
    name: &'a str,
    payload: &'a str,
    expected: GraphProposal,
    scalars: &'a [&'a str],
    strict_error: Option<&'a str>,
    latent_exact: bool,
}

#[test]
fn terra_low_randomized_matrix_separates_wire_failures_from_semantic_failures() {
    let trials = [
        Trial {
            name: "S1",
            payload: include_str!("fixtures/terra_matrix_s1.json"),
            expected: short(1),
            scalars: &["cobalt", "amber"],
            strict_error: None,
            latent_exact: true,
        },
        Trial {
            name: "S2",
            payload: include_str!("fixtures/terra_matrix_s2.json"),
            expected: short(2),
            scalars: &["north", "south"],
            strict_error: Some("$.schema"),
            latent_exact: true,
        },
        Trial {
            name: "S3",
            payload: include_str!("fixtures/terra_matrix_s3.json"),
            expected: short(3),
            scalars: &["prism", "ember"],
            strict_error: None,
            latent_exact: true,
        },
        Trial {
            name: "S4",
            payload: include_str!("fixtures/terra_matrix_s4.json"),
            expected: short(4),
            scalars: &["quartz", "lime"],
            strict_error: Some("$.schema"),
            latent_exact: true,
        },
        Trial {
            name: "R1",
            payload: include_str!("fixtures/terra_matrix_r1.json"),
            expected: recurrence(1),
            scalars: &["a0", "a1", "a2", "a3"],
            strict_error: None,
            latent_exact: true,
        },
        Trial {
            name: "R2",
            payload: include_str!("fixtures/terra_matrix_r2.json"),
            expected: recurrence(2),
            scalars: &["p0", "p1", "p2", "p3", "p4"],
            strict_error: Some("$.scalars"),
            latent_exact: false,
        },
        Trial {
            name: "R3",
            payload: include_str!("fixtures/terra_matrix_r3.json"),
            expected: recurrence(3),
            scalars: &["k0", "k1", "k2", "k3"],
            strict_error: Some("$.scalars"),
            latent_exact: true,
        },
        Trial {
            name: "R4",
            payload: include_str!("fixtures/terra_matrix_r4.json"),
            expected: recurrence(4),
            scalars: &["z0", "z1", "z2", "z3", "z4"],
            strict_error: Some("$.scalars"),
            latent_exact: false,
        },
        Trial {
            name: "D1",
            payload: include_str!("fixtures/terra_matrix_d1.json"),
            expected: design(1),
            scalars: &[],
            strict_error: None,
            latent_exact: true,
        },
        Trial {
            name: "D2",
            payload: include_str!("fixtures/terra_matrix_d2.json"),
            expected: design(2),
            scalars: &[],
            strict_error: None,
            latent_exact: true,
        },
        Trial {
            name: "D3",
            payload: include_str!("fixtures/terra_matrix_d3.json"),
            expected: design(3),
            scalars: &[],
            strict_error: None,
            latent_exact: false,
        },
        Trial {
            name: "D4",
            payload: include_str!("fixtures/terra_matrix_d4.json"),
            expected: design(4),
            scalars: &[],
            strict_error: None,
            latent_exact: true,
        },
    ];
    let mut schema_passes = 0;
    let mut exact_first_passes = 0;
    let mut latent_semantic_passes = 0;
    for trial in trials {
        match parse_proposal(trial.payload) {
            Ok(actual) => {
                assert!(
                    trial.strict_error.is_none(),
                    "{} unexpectedly passed schema",
                    trial.name
                );
                schema_passes += 1;
                if actual == trial.expected {
                    exact_first_passes += 1;
                }
            }
            Err(error) => assert_eq!(
                Some(error.path.as_str()),
                trial.strict_error,
                "{} first schema error",
                trial.name
            ),
        }
        let normalized = diagnostic_normalize(trial.payload, trial.scalars);
        assert_eq!(
            normalized == trial.expected,
            trial.latent_exact,
            "{} latent semantic classification",
            trial.name
        );
        latent_semantic_passes += usize::from(normalized == trial.expected);
    }
    assert_eq!(schema_passes, 7);
    assert_eq!(exact_first_passes, 6);
    assert_eq!(latent_semantic_passes, 9);
}

#[test]
fn d3_first_semantic_mismatch_is_the_recovery_lag_at_operation_29() {
    let actual = parse_proposal(include_str!("fixtures/terra_matrix_d3.json"))
        .expect("D3 has a strict wire shape");
    let expected = design(3);
    assert_eq!(actual.operations[..29], expected.operations[..29]);
    assert_eq!(actual.operations[29].operands[1], local(5));
    assert_eq!(expected.operations[29].operands[1], local(11));
}

#[test]
fn one_shot_local_repairs_recover_all_six_failed_first_attempts() {
    let repaired = [
        (
            "S2",
            include_str!("fixtures/terra_matrix_s2_repaired.json"),
            short(2),
        ),
        (
            "S4",
            include_str!("fixtures/terra_matrix_s4_repaired.json"),
            short(4),
        ),
        (
            "R2",
            include_str!("fixtures/terra_matrix_r2_repaired.json"),
            recurrence(2),
        ),
        (
            "R3",
            include_str!("fixtures/terra_matrix_r3_repaired.json"),
            recurrence(3),
        ),
        (
            "R4",
            include_str!("fixtures/terra_matrix_r4_repaired.json"),
            recurrence(4),
        ),
        (
            "D3",
            include_str!("fixtures/terra_matrix_d3_repaired.json"),
            design(3),
        ),
    ];
    for (name, payload, expected) in repaired {
        let actual = parse_proposal(payload)
            .unwrap_or_else(|error| panic!("{name} repair must satisfy strict schema: {error}"));
        assert_eq!(actual, expected, "{name} one-shot repair must match intent");
    }
}
