use russian_draughts_classic::{EvalParams, PhaseScore, BASE_EVAL_TERM_COUNT, MAN_PSQT_OFFSET};

#[test]
fn eval_params_kv_round_trip() {
    let original = EvalParams::default();
    let text = original.to_kv_string();
    let parsed = EvalParams::parse_kv(&text).expect("default parameters must round-trip");
    assert_eq!(parsed, original);
}

#[test]
fn production_eval_file_matches_embedded_default() {
    let parsed = EvalParams::parse_kv(include_str!("../config/eval_default.txt"))
        .expect("production eval config must parse");
    assert_eq!(parsed, EvalParams::default());
}

#[test]
fn legacy_eval_file_maps_scalar_weights_to_both_phases() {
    let parsed = EvalParams::parse_kv(
        "man_value=100\nking_value=301\nadvancement=0,1,2,3,4,5,6,0\nmobility=7\n",
    )
    .expect("v1.2 scalar file must remain loadable");
    assert_eq!(parsed.term(0), PhaseScore::same(100));
    assert_eq!(parsed.term(1), PhaseScore::same(301));
    assert_eq!(parsed.term(14), PhaseScore::same(7));
    assert_eq!(parsed.term(MAN_PSQT_OFFSET), PhaseScore::same(0));
}

#[test]
fn eval_params_reject_unknown_keys() {
    let err = EvalParams::parse_kv("mobillity=3\n").expect_err("typo must not be ignored");
    assert!(err.contains("unknown evaluation key"));
}

#[test]
fn tapered_file_rejects_missing_parameters() {
    let err = EvalParams::parse_kv("format=tapered_v1\nman_value_mg=100\nman_value_eg=100\n")
        .expect_err("declared tapered file must be complete");
    assert!(err.contains("incomplete"));
}

#[test]
fn tapered_file_rejects_legacy_scalar_keys() {
    let mut text = EvalParams::default().to_kv_string();
    text.push_str("mobility=3\n");
    let err = EvalParams::parse_kv(&text).expect_err("mixed declared format must be rejected");
    assert!(err.contains("legacy scalar") || err.contains("duplicate"));
}

#[test]
fn eval_params_reject_duplicate_parameter_assignment() {
    let err = EvalParams::parse_kv("mobility=3\nmobility=4\n")
        .expect_err("duplicate parameter must not be order-dependent");
    assert!(err.contains("duplicate"));
}

#[test]
fn tapered_v1_file_loads_with_new_v14_terms_zeroed() {
    let mut legacy = EvalParams::default().to_kv_string();
    legacy = legacy.replace("format=tapered_v2", "format=tapered_v1");
    for key in [
        "isolated_man_penalty",
        "supported_man",
        "edge_isolated_man_penalty",
        "wing_imbalance_penalty",
        "advanced_supported_man",
        "man_quiet_mobility",
        "promotion_move",
        "constrained_king_penalty",
    ] {
        legacy = legacy
            .lines()
            .filter(|line| {
                !line.starts_with(&format!("{key}_mg=")) && !line.starts_with(&format!("{key}_eg="))
            })
            .collect::<Vec<_>>()
            .join("\n");
        legacy.push('\n');
    }
    let parsed = EvalParams::parse_kv(&legacy).expect("v1.3 tapered file must remain loadable");
    for term in 20..BASE_EVAL_TERM_COUNT {
        assert_eq!(parsed.term(term), PhaseScore::same(0));
    }
}

#[test]
fn tapered_v2_file_requires_new_v14_terms() {
    let text = EvalParams::default().to_kv_string();
    let broken = text
        .lines()
        .filter(|line| !line.starts_with("isolated_man_penalty_eg="))
        .collect::<Vec<_>>()
        .join("\n");
    let err = EvalParams::parse_kv(&broken).expect_err("v2 file with missing new term must fail");
    assert!(err.contains("incomplete"));
    assert!(err.contains("isolated_man_penalty_eg"));
}

#[test]
fn tapered_v2_file_requires_legacy_terms_too() {
    let text = EvalParams::default().to_kv_string();
    let broken = text
        .lines()
        .filter(|line| !line.starts_with("mobility_mg="))
        .collect::<Vec<_>>()
        .join("\n");
    let err =
        EvalParams::parse_kv(&broken).expect_err("v2 file with missing legacy term must fail");
    assert!(err.contains("incomplete"));
    assert!(err.contains("mobility_mg"));
}
