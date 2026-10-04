use russian_draughts_classic::{
    generate_moves, square, ClassicEngine, ClassicEngineConfig, ClassicSearchDecision, EvalParams,
    Game, GameStatus, Move, Position, SearchLimits, SearchParams, Side,
    DEFAULT_MOVE_BUFFER_CAPACITY,
};

fn s(name: &str) -> u8 {
    let b = name.as_bytes();
    square(b[0] - b'a', b[1] - b'1').unwrap()
}

fn bb(names: &[&str]) -> u32 {
    names
        .iter()
        .fold(0u32, |acc, &name| acc | (1u32 << s(name)))
}

fn position(white: &[&str], black: &[&str], kings: &[&str], side: Side) -> Position {
    Position {
        white: bb(white),
        black: bb(black),
        kings: bb(kings),
        side_to_move: side,
    }
}

fn find_move(game: &Game, from: &str, to: &str) -> Move {
    let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = generate_moves(game.position(), &mut moves);
    moves[..len]
        .iter()
        .copied()
        .find(|m| m.from() == s(from) && m.to() == s(to))
        .unwrap_or_else(|| panic!("move {from}-{to} is not legal"))
}

#[test]
fn evaluation_values_material_and_kings() {
    let engine = ClassicEngine::default();

    let man_game = Game::new(position(&["c3"], &[], &[], Side::White));
    let king_game = Game::new(position(&["c3"], &[], &["c3"], Side::White));
    let man_eval = engine.evaluate(&man_game);
    let king_eval = engine.evaluate(&king_game);

    assert!(man_eval.total_white > 0);
    assert!(king_eval.total_white > man_eval.total_white);
}

#[test]
fn search_finds_the_only_forced_capture_and_win() {
    let game = Game::new(position(&["a3"], &["b4"], &[], Side::White));
    let mut engine = ClassicEngine::default();
    let result = engine.search(&game, SearchLimits::depth(6));

    match result.decision {
        ClassicSearchDecision::Move(mv) => {
            assert_eq!(mv.from(), s("a3"));
            assert_eq!(mv.to(), s("c5"));
            assert!(mv.is_capture());
        }
        other => panic!("unexpected decision: {other:?}"),
    }
    assert!(result.score > 29_000);
}

#[test]
fn search_does_not_mutate_root_game() {
    let game = Game::initial();
    let before = game.position();
    let before_hash = game.zobrist();
    let mut engine = ClassicEngine::default();
    let _ = engine.search(&game, SearchLimits::depth(4));
    assert_eq!(game.position(), before);
    assert_eq!(game.zobrist(), before_hash);
}

#[test]
fn initial_search_returns_a_legal_move_deterministically() {
    let game = Game::initial();
    let mut engine_a = ClassicEngine::default();
    let mut engine_b = ClassicEngine::default();
    let first = engine_a.search(&game, SearchLimits::depth(4));
    let second = engine_b.search(&game, SearchLimits::depth(4));
    assert_eq!(first.decision, second.decision);
    assert_eq!(first.score, second.score);

    let mv = first
        .best_move()
        .expect("initial position must produce a move");
    let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = generate_moves(game.position(), &mut legal);
    assert!(legal[..len].contains(&mv));
}

#[test]
fn threefold_claim_is_a_real_search_action() {
    let mut game = Game::new(position(
        &["a1", "h6"],
        &["h2", "a7"],
        &["a1", "h2"],
        Side::White,
    ));
    for _ in 0..2 {
        let m1 = find_move(&game, "a1", "b2");
        game.play(m1);
        let m2 = find_move(&game, "h2", "g1");
        game.play(m2);
        let m3 = find_move(&game, "b2", "a1");
        game.play(m3);
        let m4 = find_move(&game, "g1", "h2");
        game.play(m4);
    }
    assert!(matches!(game.status(), GameStatus::ClaimableDraw(_)));

    let mut params = EvalParams::default();
    params.clear_non_material_terms();
    let config = ClassicEngineConfig {
        eval: params,
        ..ClassicEngineConfig::default()
    };
    let mut engine = ClassicEngine::new(config);
    let result = engine.search(&game, SearchLimits::depth(1));
    assert_eq!(result.decision, ClassicSearchDecision::ClaimDraw);
    assert_eq!(result.score, 0);
}

#[test]
fn declared_third_repetition_is_a_search_action() {
    let mut game = Game::new(position(
        &["a1", "h6"],
        &["h2", "a7"],
        &["a1", "h2"],
        Side::White,
    ));

    // One complete cycle gives the initial position its second occurrence.
    for (from, to) in [("a1", "b2"), ("h2", "g1"), ("b2", "a1"), ("g1", "h2")] {
        let mv = find_move(&game, from, to);
        game.play(mv);
    }
    // Stop one ply before Black can declare g1-h2 as the third occurrence.
    for (from, to) in [("a1", "b2"), ("h2", "g1"), ("b2", "a1")] {
        let mv = find_move(&game, from, to);
        game.play(mv);
    }
    assert!(matches!(game.status(), GameStatus::Ongoing));
    let declared = find_move(&game, "g1", "h2");
    assert!(game.move_creates_third_repetition(declared));

    let mut params = EvalParams::default();
    params.clear_non_material_terms();
    let mut engine = ClassicEngine::new(ClassicEngineConfig {
        eval: params,
        ..ClassicEngineConfig::default()
    });
    let result = engine.search(&game, SearchLimits::depth(1));
    assert_eq!(
        result.decision,
        ClassicSearchDecision::ClaimDrawByMove(declared)
    );
    assert_eq!(result.score, 0);
}

#[test]
fn evaluation_is_color_symmetric_under_180_degree_rotation() {
    let engine = ClassicEngine::default();
    let p = position(
        &["a1", "c3", "f4", "h6"],
        &["b8", "d6", "g5"],
        &["a1", "d6"],
        Side::White,
    );
    let rotated = Position {
        white: p.black.reverse_bits(),
        black: p.white.reverse_bits(),
        kings: p.kings.reverse_bits(),
        side_to_move: Side::Black,
    };
    let a = engine.evaluate(&Game::new(p));
    let b = engine.evaluate(&Game::new(rotated));
    assert_eq!(a.total_white, -b.total_white);
    assert_eq!(a.side_to_move, b.side_to_move);
}

fn assert_pv_is_legal(root: &Game, pv: &[Move]) {
    let mut game = root.clone();
    for (ply, &mv) in pv.iter().enumerate() {
        let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let len = generate_moves(game.position(), &mut legal);
        assert!(
            legal[..len].contains(&mv),
            "illegal PV move at ply {ply}: raw=0x{:016x}",
            mv.raw()
        );
        game.play(mv);
    }
}

#[test]
fn returned_pv_is_legal_from_initial_position() {
    let game = Game::initial();
    let mut engine = ClassicEngine::default();
    let result = engine.search(&game, SearchLimits::depth(5));
    assert!(result.stats.pv_truncations == 0);
    assert_pv_is_legal(&game, &result.principal_variation);
    if let ClassicSearchDecision::Move(best) = result.decision {
        assert_eq!(result.principal_variation.first().copied(), Some(best));
    }
}

#[test]
fn returned_pv_is_legal_on_diagnostic_corpus() {
    for case in russian_draughts_classic::benchmark_suite() {
        let mut engine = ClassicEngine::default();
        let result = engine.search(&case.game, SearchLimits::depth(5));
        assert_eq!(
            result.stats.pv_truncations, 0,
            "PV validator had to truncate case {}",
            case.name
        );
        assert_pv_is_legal(&case.game, &result.principal_variation);
    }
}

#[test]
fn node_limit_returns_a_legal_fallback_before_first_iteration_completes() {
    let game = Game::initial();
    let mut engine = ClassicEngine::default();
    let result = engine.search(&game, SearchLimits::nodes(32, 1));
    assert_eq!(result.depth, 0);
    assert_eq!(
        result.stop_reason,
        russian_draughts_classic::ClassicSearchStopReason::NodeLimit
    );
    let mv = result.best_move().expect("fallback move must be available");
    let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = generate_moves(game.position(), &mut legal);
    assert!(legal[..len].contains(&mv));
}

#[test]
fn zero_time_limit_returns_a_legal_fallback() {
    let game = Game::initial();
    let config = ClassicEngineConfig {
        time_check_interval: 1,
        ..ClassicEngineConfig::default()
    };
    let mut engine = ClassicEngine::new(config);
    let result = engine.search(&game, SearchLimits::timed(32, std::time::Duration::ZERO));
    assert_eq!(result.depth, 0);
    assert_eq!(
        result.stop_reason,
        russian_draughts_classic::ClassicSearchStopReason::TimeLimit
    );
    let mv = result.best_move().expect("fallback move must be available");
    let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = generate_moves(game.position(), &mut legal);
    assert!(legal[..len].contains(&mv));
}

#[test]
fn external_stop_interrupts_search_and_keeps_completed_or_fallback_move() {
    let game = Game::initial();
    let config = ClassicEngineConfig {
        time_check_interval: 1,
        ..ClassicEngineConfig::default()
    };
    let mut engine = ClassicEngine::new(config);
    let control = engine.control();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2));
        control.request_stop();
    });
    let result = engine.search(&game, SearchLimits::depth(64));
    stopper.join().expect("stopper thread must finish");
    assert_eq!(
        result.stop_reason,
        russian_draughts_classic::ClassicSearchStopReason::ExternalStop
    );
    let mv = result
        .best_move()
        .expect("interrupted search must retain a legal move");
    let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = generate_moves(game.position(), &mut legal);
    assert!(legal[..len].contains(&mv));
}

#[test]
fn optimized_search_matches_reference_at_shallow_depth_without_extensions_or_lmr() {
    let search = SearchParams {
        max_extensions_per_line: 0,
        enable_lmr: false,
        aspiration_window: 32_000,
        ..SearchParams::default()
    };
    let config = ClassicEngineConfig {
        search,
        ..ClassicEngineConfig::default()
    };

    for case in russian_draughts_classic::benchmark_suite() {
        let mut optimized = ClassicEngine::new(config);
        let got = optimized.search(&case.game, SearchLimits::depth(3));
        let mut reference = russian_draughts_classic::ReferenceSearcher::new(config.eval);
        let expected = reference.search(&case.game, 3);
        assert_eq!(
            got.score, expected.score,
            "optimized/reference score mismatch in {}",
            case.name
        );
    }
}
