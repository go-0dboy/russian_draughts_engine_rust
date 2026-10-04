use russian_draughts_classic::{
    api::{
        checked_position, legal_moves, resolve_legal_move, CheckedGame, MoveResolutionError,
        MoveSpec,
    },
    ClassicEngine, ClassicEngineBuildError, ClassicEngineConfig, Position, SearchParams, Side,
};

#[test]
fn checked_position_rejects_overlap() {
    assert!(checked_position(1, 1, 0, Side::White).is_err());
}

#[test]
fn product_legal_moves_match_initial_contract() {
    let moves = legal_moves(Position::initial()).expect("initial position must be valid");
    assert_eq!(moves.len(), 7);
}

#[test]
fn move_spec_round_trip_resolves_canonical_move() {
    let moves = legal_moves(Position::initial()).unwrap();
    let spec = moves
        .iter()
        .next()
        .expect("initial position has legal moves");
    let resolved = resolve_legal_move(Position::initial(), spec).unwrap();
    assert_eq!(MoveSpec::from_move(resolved), spec);
}

#[test]
fn checked_game_rejects_illegal_external_move() {
    let mut game = CheckedGame::initial();
    let err = game
        .play(MoveSpec {
            from: 31,
            to: 0,
            captures: 0,
        })
        .unwrap_err();
    assert_eq!(err, MoveResolutionError::IllegalMove);
}

#[test]
fn checked_game_rejects_move_after_terminal_status() {
    let position = checked_position(0, 1, 0, Side::White).unwrap();
    let mut game = CheckedGame::new(position).unwrap();
    let err = game
        .play(MoveSpec {
            from: 0,
            to: 4,
            captures: 0,
        })
        .unwrap_err();
    assert_eq!(err, MoveResolutionError::GameFinished);
}

#[test]
fn engine_try_new_rejects_invalid_config_without_panicking() {
    let search = SearchParams {
        aspiration_window: 1,
        ..SearchParams::default()
    };
    let err = ClassicEngine::try_new(ClassicEngineConfig {
        search,
        ..ClassicEngineConfig::default()
    })
    .err()
    .expect("invalid config must be rejected");
    assert!(matches!(
        err,
        ClassicEngineBuildError::InvalidSearchParams(_)
    ));
}
