use russian_draughts_classic::{ClassicEngineConfig, SearchParams};

#[test]
fn search_params_round_trip() {
    let original = SearchParams::default();
    let text = original.to_kv_string();
    let parsed = SearchParams::parse_kv(&text).expect("default search params must round-trip");
    assert_eq!(parsed, original);
}

#[test]
fn production_search_file_is_the_single_default_source() {
    let production_text = include_str!("../config/search_default.txt");
    let parsed =
        SearchParams::parse_kv(production_text).expect("production search config must parse");
    assert_eq!(SearchParams::default(), parsed);
    assert_eq!(SearchParams::default().to_kv_string(), production_text);
}

#[test]
fn search_params_require_version_and_all_fields() {
    let err = SearchParams::parse_kv("aspiration_window=48\n")
        .expect_err("unversioned search config must be rejected");
    assert!(err.contains("format=search_v1"));

    let broken = SearchParams::default()
        .to_kv_string()
        .lines()
        .filter(|line| !line.starts_with("lmr_second_move="))
        .collect::<Vec<_>>()
        .join("\n");
    let err =
        SearchParams::parse_kv(&broken).expect_err("search_v1 with missing parameter must fail");
    assert!(err.contains("lmr_second_move"));
}

#[test]
fn search_params_reject_unknown_duplicate_and_invalid_values() {
    let mut duplicate = SearchParams::default().to_kv_string();
    duplicate.push_str("aspiration_window=64\n");
    assert!(SearchParams::parse_kv(&duplicate)
        .expect_err("duplicate must fail")
        .contains("duplicate"));

    let mut unknown = SearchParams::default().to_kv_string();
    unknown.push_str("magic_pruning=1\n");
    assert!(SearchParams::parse_kv(&unknown)
        .expect_err("unknown key must fail")
        .contains("unknown search parameter"));

    let invalid = SearchParams::default()
        .to_kv_string()
        .replace("lmr_min_depth=3", "lmr_min_depth=1");
    assert!(SearchParams::parse_kv(&invalid)
        .expect_err("invalid value must fail")
        .contains("lmr_min_depth"));
}

#[test]
fn default_engine_uses_production_search_config() {
    let production = SearchParams::parse_kv(include_str!("../config/search_default.txt"))
        .expect("production search config must parse");
    let config = ClassicEngineConfig::default();
    assert_eq!(config.search, production);
    assert_eq!(config.search, SearchParams::default());
}

#[test]
fn programmatic_search_params_can_be_validated_before_engine_creation() {
    let params = SearchParams {
        history_malus_divisor: 0,
        ..SearchParams::default()
    };
    assert!(params.validate().is_err());
}
