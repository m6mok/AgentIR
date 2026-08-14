use agentir_authoring::{
    GRAPH_SCHEMA, GraphOpcode as O, GraphOperand as A, GraphOperation, GraphProposal,
    parse_proposal,
};
use serde_json::{Value, json};

fn s(name: impl Into<String>) -> A {
    A::Scalar { name: name.into() }
}
fn t(name: impl Into<String>) -> A {
    A::Tensor { name: name.into() }
}
const fn l(operation: usize) -> A {
    A::Local { operation }
}
fn o(op: O, operands: Vec<A>) -> GraphOperation {
    GraphOperation { op, operands }
}
fn p(operations: Vec<GraphOperation>, r#yield: usize) -> GraphProposal {
    GraphProposal {
        schema: GRAPH_SCHEMA.into(),
        operations,
        r#yield,
    }
}

fn a_seed() -> Vec<GraphOperation> {
    vec![
        o(O::Mul, vec![t("x0"), s("a0")]),
        o(O::Fma, vec![t("x1"), s("a1"), t("x2")]),
        o(O::Add, vec![l(1), l(0)]),
        o(O::Mul, vec![l(2), t("x3")]),
        o(O::Add, vec![t("x4"), l(0)]),
    ]
}
fn a_rec(len: usize, modulus: usize) -> GraphProposal {
    let mut v = a_seed();
    for n in 5..len {
        v.push(match modulus {
            5 => match n % 5 {
                0 => o(
                    O::Fma,
                    vec![l(n - 1), s(format!("a{}", (n / 5) % 5)), l(n - 5)],
                ),
                1 => o(O::Add, vec![t(format!("x{}", (n + 3) % 10)), l(n - 1)]),
                2 => o(O::Mul, vec![l(n - 1), t(format!("x{}", (2 * n + 1) % 10))]),
                3 => o(O::Add, vec![l(n - 1), l(n - 3)]),
                _ => o(
                    O::Fma,
                    vec![s(format!("a{}", (n + 1) % 5)), l(n - 1), l(n - 4)],
                ),
            },
            6 => match n % 6 {
                0 => o(O::Add, vec![l(n - 1), l(n - 5)]),
                1 => o(O::Mul, vec![t(format!("x{}", (3 * n + 2) % 10)), l(n - 1)]),
                2 => o(
                    O::Fma,
                    vec![s(format!("a{}", (n / 2) % 5)), l(n - 1), l(n - 7)],
                ),
                3 => o(O::Add, vec![t(format!("x{}", (n + 4) % 10)), l(n - 1)]),
                4 => o(O::Mul, vec![l(n - 1), s(format!("a{}", (n + 3) % 5))]),
                _ => o(
                    O::Fma,
                    vec![l(n - 1), t(format!("x{}", (2 * n) % 10)), l(n - 4)],
                ),
            },
            7 => match n % 7 {
                0 => o(O::Add, vec![l(n - 1), l(n - 5)]),
                1 => o(O::Mul, vec![t(format!("x{}", (n + 1) % 10)), l(n - 1)]),
                2 => o(
                    O::Fma,
                    vec![s(format!("a{}", (n / 7) % 5)), l(n - 1), l(n - 8)],
                ),
                3 => o(O::Add, vec![l(n - 1), t(format!("x{}", (4 * n + 3) % 10))]),
                4 => o(O::Mul, vec![s(format!("a{}", (2 * n + 1) % 5)), l(n - 1)]),
                5 => o(
                    O::Fma,
                    vec![l(n - 1), s(format!("a{}", (n + 4) % 5)), l(n - 3)],
                ),
                _ => o(O::Add, vec![l(n - 1), l(n / 2)]),
            },
            _ => unreachable!(),
        });
    }
    p(v, len - 1)
}
fn design(prefix_s: &str, prefix_t: &str, variant: usize) -> GraphProposal {
    let mut v = Vec::new();
    for i in 0..16 {
        let b = i * 6;
        let state = if i == 0 {
            t(format!("{prefix_t}0"))
        } else {
            l(b - 1)
        };
        let (ai, bi, mi, si, fi) = if variant == 0 {
            (
                i % 5,
                (2 * i + 1) % 10,
                (3 * i + 2) % 10,
                (5 * i + 3) % 10,
                (i + 2) % 5,
            )
        } else {
            (
                (2 * i + 1) % 5,
                (3 * i + 2) % 10,
                (4 * i + 1) % 10,
                (5 * i + 4) % 10,
                (i + 3) % 5,
            )
        };
        v.extend([
            o(
                O::Fma,
                vec![
                    s(format!("{prefix_s}{ai}")),
                    state.clone(),
                    t(format!("{prefix_t}{bi}")),
                ],
            ),
            o(O::Mul, vec![t(format!("{prefix_t}{mi}")), l(b)]),
            o(O::Add, vec![t(format!("{prefix_t}{si}")), l(b)]),
            o(O::Add, vec![l(b + 1), l(b + 2)]),
            o(O::Fma, vec![l(b + 3), s(format!("{prefix_s}{fi}")), state]),
            o(
                O::Add,
                vec![
                    l(b + 4),
                    if i < 4 {
                        t(format!("{prefix_t}{}", 9 - i))
                    } else {
                        l((i - 3) * 6 + 5)
                    },
                ],
            ),
        ]);
    }
    p(v, 95)
}

fn b_seed() -> Vec<GraphOperation> {
    vec![
        o(O::Add, vec![t("t0"), t("t1")]),
        o(O::Mul, vec![s("p0"), l(0)]),
        o(O::Fma, vec![t("t2"), s("p1"), l(1)]),
        o(O::Add, vec![l(2), l(0)]),
    ]
}
fn b_rec(len: usize) -> GraphProposal {
    let mut v = b_seed();
    for n in 4..len {
        v.push(match n % 4 {
            0 => o(O::Mul, vec![l(n - 1), l(0)]),
            1 => o(O::Add, vec![t(format!("t{}", (n + 2) % 10)), l(n - 1)]),
            2 => o(O::Fma, vec![s(format!("p{}", n % 5)), l(n - 1), l(n / 2)]),
            _ => o(O::Add, vec![l(n - 1), l(n - 3)]),
        });
    }
    p(v, len - 1)
}
fn c_seed() -> Vec<GraphOperation> {
    vec![
        o(O::Fma, vec![t("data2"), s("k2"), t("data3")]),
        o(O::Mul, vec![t("data4"), l(0)]),
        o(O::Add, vec![t("data5"), l(0)]),
        o(O::Add, vec![l(1), l(2)]),
        o(O::Fma, vec![l(3), s("k3"), l(0)]),
        o(O::Add, vec![l(4), l(2)]),
    ]
}
fn c_rec(len: usize, wide: bool, y: usize) -> GraphProposal {
    let mut v = c_seed();
    for n in 6..len {
        v.push(if wide {
            match n % 6 {
                0 => o(O::Add, vec![l(n - 1), l(n - 6)]),
                1 => o(
                    O::Fma,
                    vec![
                        s(format!("k{}", (n + 2) % 5)),
                        l(n - 1),
                        t(format!("data{}", (n + 3) % 10)),
                    ],
                ),
                2 => o(O::Mul, vec![t(format!("data{}", (2 * n) % 10)), l(n - 1)]),
                3 => o(O::Add, vec![l(n - 1), l(n / 3)]),
                4 => o(
                    O::Fma,
                    vec![l(n - 1), s(format!("k{}", (n + 1) % 5)), l(n - 4)],
                ),
                _ => o(O::Mul, vec![l(n - 1), t(format!("data{}", (n + 7) % 10))]),
            }
        } else {
            match n % 3 {
                0 => o(O::Add, vec![l(n - 1), l(n - 6)]),
                1 => o(
                    O::Fma,
                    vec![
                        s(format!("k{}", (n + 2) % 5)),
                        l(n - 1),
                        t(format!("data{}", (n + 3) % 10)),
                    ],
                ),
                _ => o(O::Mul, vec![t(format!("data{}", (2 * n) % 10)), l(n - 1)]),
            }
        });
    }
    p(v, y)
}

fn expected(id: &str) -> GraphProposal {
    match id {
        "A1" => p(vec![o(O::Fma, vec![t("x0"), s("a0"), t("x1")])], 0),
        "A2" => p(
            vec![
                o(O::Add, vec![t("x2"), t("x2")]),
                o(O::Mul, vec![l(0), s("a1")]),
            ],
            0,
        ),
        "A3" => p(
            vec![
                o(O::Fma, vec![s("a2"), t("x3"), t("x4")]),
                o(O::Add, vec![l(0), t("x5")]),
                o(O::Mul, vec![t("x6"), l(0)]),
                o(O::Add, vec![l(1), l(2)]),
                o(O::Fma, vec![l(3), s("a3"), l(0)]),
            ],
            4,
        ),
        "A4" => {
            let mut v = a_seed();
            v.extend([
                o(O::Fma, vec![l(4), s("a2"), l(3)]),
                o(O::Mul, vec![l(5), l(1)]),
                o(O::Add, vec![t("x5"), l(6)]),
                o(O::Fma, vec![s("a3"), l(7), l(2)]),
                o(O::Add, vec![l(8), l(0)]),
                o(O::Mul, vec![t("x6"), l(9)]),
                o(O::Add, vec![l(10), l(5)]),
            ]);
            p(v, 11)
        }
        "A5" => a_rec(31, 5),
        "A6" => a_rec(64, 6),
        "A7" => design("a", "x", 0),
        "A8" => a_rec(128, 7),
        "B1" => p(
            vec![
                o(O::Mul, vec![s("p0"), t("t0")]),
                o(O::Add, vec![t("t1"), l(0)]),
                o(O::Fma, vec![l(1), s("p1"), l(0)]),
                o(O::Mul, vec![l(2), l(2)]),
            ],
            3,
        ),
        "B2" => p(
            vec![
                o(O::Fma, vec![t("t0"), s("p0"), t("t1")]),
                o(O::Fma, vec![s("p1"), t("t2"), l(0)]),
                o(O::Fma, vec![l(1), t("t3"), l(0)]),
            ],
            2,
        ),
        "B3" => b_rec(20),
        "B4" => b_rec(50),
        "B5" => b_rec(99),
        "B6" => b_rec(100),
        "B7" => b_rec(101),
        "B8" => b_rec(127),
        "C1" => p(
            vec![
                o(O::Add, vec![t("data0"), t("data1")]),
                o(O::Mul, vec![s("k0"), l(0)]),
                o(O::Add, vec![l(0), l(1)]),
                o(O::Fma, vec![l(2), s("k1"), l(0)]),
                o(O::Add, vec![l(3), l(3)]),
            ],
            4,
        ),
        "C2" => p(c_seed(), 5),
        "C3" => c_rec(24, false, 23),
        "C4" => c_rec(72, false, 71),
        "C5" => design("k", "data", 1),
        "C6" => c_rec(120, true, 119),
        "C7" => c_rec(128, true, 127),
        "C8" => c_rec(24, false, 0),
        _ => unreachable!(),
    }
}

fn normalize(mut v: Value, campaign: &str) -> GraphProposal {
    let root = v.as_object_mut().unwrap();
    root.insert("schema".into(), json!(GRAPH_SCHEMA));
    if let Some(y) = root["yield"].as_object() {
        root["yield"] = y.get("index").cloned().unwrap();
    }
    for op in root["operations"].as_array_mut().unwrap() {
        for a in op["operands"].as_array_mut().unwrap() {
            let x = a.as_object_mut().unwrap();
            if let Some(kind) = x.remove("type") {
                x.insert("kind".into(), kind);
            }
            let kind = x["kind"].as_str().unwrap().to_owned();
            if let Some(index) = x.remove("index") {
                if kind == "local" {
                    x.insert("operation".into(), index);
                } else {
                    let pre = match (campaign, kind.as_str()) {
                        ("B", "scalar") => "p",
                        ("B", "tensor") => "t",
                        _ => unreachable!(),
                    };
                    x.insert(
                        "name".into(),
                        json!(format!("{pre}{}", index.as_u64().unwrap())),
                    );
                }
            }
            if kind == "local" && !x.contains_key("operation") {
                let n = x
                    .remove("name")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .trim_start_matches('l')
                    .parse::<usize>()
                    .unwrap();
                x.insert("operation".into(), json!(n));
            }
        }
    }
    parse_proposal(&v.to_string()).unwrap()
}

#[test]
fn luna_medium_first_attempt_matrix() {
    let files = [
        ("A", include_str!("fixtures/luna_medium_edge_a.json")),
        ("B", include_str!("fixtures/luna_medium_edge_b.json")),
        ("C", include_str!("fixtures/luna_medium_edge_c.json")),
    ];
    let mut strict = 0;
    let mut latent = 0;
    for (c, text) in files {
        let batch: Value = serde_json::from_str(text).unwrap();
        for case in batch["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let raw = case["proposal"].to_string();
            strict += usize::from(parse_proposal(&raw).is_ok());
            let got = normalize(case["proposal"].clone(), c);
            let exact = got == expected(id);
            eprintln!(
                "{id}: strict={} latent={exact}",
                parse_proposal(&raw).is_ok()
            );
            latent += usize::from(exact);
        }
    }
    eprintln!("TOTAL strict={strict}/24 latent={latent}/24");
    assert_eq!(strict, 0);
    assert!(
        latent >= 16,
        "Luna should preserve most semantics beneath dialect drift"
    );
}

#[test]
fn luna_medium_stateless_batch_repairs_preserve_all_twenty_four_intents() {
    let files = [
        include_str!("fixtures/luna_medium_edge_a_repaired.json"),
        include_str!("fixtures/luna_medium_edge_b_repaired.json"),
        include_str!("fixtures/luna_medium_edge_c_repaired.json"),
    ];
    let mut exact = 0;
    for text in files {
        let batch: Value = serde_json::from_str(text).expect("valid repaired batch JSON");
        for case in batch["cases"].as_array().expect("cases") {
            let id = case["id"].as_str().expect("case ID");
            let actual = parse_proposal(&case["proposal"].to_string())
                .unwrap_or_else(|error| panic!("{id} strict repaired proposal: {error}"));
            assert_eq!(actual, expected(id), "{id} repaired intent");
            exact += 1;
        }
    }
    assert_eq!(exact, 24);
}

#[test]
fn luna_medium_literal_wire_example_fixes_schema_but_not_design_lag_reasoning() {
    let recurrence_101 = parse_proposal(include_str!("fixtures/luna_medium_single_101.json"))
        .expect("101-operation control must pass strict schema");
    assert_eq!(recurrence_101, b_rec(101));

    let recurrence_128 = parse_proposal(include_str!("fixtures/luna_medium_single_128.json"))
        .expect("128-operation control must pass strict schema");
    assert_eq!(recurrence_128, c_rec(128, true, 127));

    let design_actual = parse_proposal(include_str!("fixtures/luna_medium_single_design96.json"))
        .expect("96-operation control must pass strict schema");
    let design_expected = design("a", "x", 0);
    assert_eq!(
        design_actual.operations[..29],
        design_expected.operations[..29]
    );
    assert_eq!(design_actual.operations[29].operands[1], l(23));
    assert_eq!(design_expected.operations[29].operands[1], l(11));
}
