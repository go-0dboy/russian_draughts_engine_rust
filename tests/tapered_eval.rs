use russian_draughts_classic::{
    evaluation_phase, ClassicEngine, ClassicEngineConfig, EvalParams, Game, PhaseScore, Position,
    Side, KING_PSQT_OFFSET, MAN_PSQT_OFFSET, PHASE_SCALE,
};

#[test]
fn phase_moves_from_initial_to_endgame() {
    assert_eq!(evaluation_phase(Position::initial()), PHASE_SCALE);
    let endgame = Position {
        white: 1 << 4,
        black: 1 << 27,
        kings: (1 << 4) | (1 << 27),
        side_to_move: Side::White,
    };
    assert_eq!(evaluation_phase(endgame), 0);
}

#[test]
fn tapered_psqt_is_color_symmetric_under_rotation() {
    let mut params = EvalParams::default();
    *params.term_mut(MAN_PSQT_OFFSET + 5) = PhaseScore::new(17, -9);
    *params.term_mut(KING_PSQT_OFFSET + 11) = PhaseScore::new(-13, 21);
    let engine = ClassicEngine::new(ClassicEngineConfig {
        eval: params,
        ..ClassicEngineConfig::default()
    });

    let p = Position {
        white: (1 << 5) | (1 << 11),
        black: (1 << 20) | (1 << 26),
        kings: (1 << 11) | (1 << 20),
        side_to_move: Side::White,
    };
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

#[test]
fn evaluator_total_matches_feature_score_for_tapered_params() {
    let mut params = EvalParams::default();
    *params.term_mut(14) = PhaseScore::new(7, -2);
    *params.term_mut(MAN_PSQT_OFFSET + 9) = PhaseScore::new(11, -5);
    *params.term_mut(KING_PSQT_OFFSET + 18) = PhaseScore::new(-8, 17);
    let engine = ClassicEngine::new(ClassicEngineConfig {
        eval: params,
        ..ClassicEngineConfig::default()
    });
    let game = Game::new(Position {
        white: (1 << 2) | (1 << 9) | (1 << 18),
        black: (1 << 21) | (1 << 29),
        kings: (1 << 18) | (1 << 21),
        side_to_move: Side::White,
    });
    let eval = engine.evaluate(&game);
    let features = engine.eval_features(&game);
    assert_eq!(eval.total_white, features.score_white(params));
}

#[test]
fn fast_score_matches_diagnostic_score_for_tapered_params_on_random_positions() {
    let mut params = EvalParams::default();
    *params.term_mut(0) = PhaseScore::new(100, 104);
    *params.term_mut(1) = PhaseScore::new(286, 302);
    *params.term_mut(10) = PhaseScore::new(2, 6);
    *params.term_mut(14) = PhaseScore::new(1, 5);
    *params.term_mut(15) = PhaseScore::new(3, 7);
    *params.term_mut(20) = PhaseScore::new(-2, 3);
    *params.term_mut(21) = PhaseScore::new(4, 8);
    *params.term_mut(22) = PhaseScore::new(5, 2);
    *params.term_mut(23) = PhaseScore::new(3, 6);
    *params.term_mut(24) = PhaseScore::new(2, 5);
    *params.term_mut(25) = PhaseScore::new(7, 12);
    *params.term_mut(26) = PhaseScore::new(4, 9);
    *params.term_mut(27) = PhaseScore::new(6, 11);
    *params.term_mut(MAN_PSQT_OFFSET) = PhaseScore::new(-1, 2);
    *params.term_mut(MAN_PSQT_OFFSET + 15) = PhaseScore::new(2, -1);
    *params.term_mut(KING_PSQT_OFFSET + 7) = PhaseScore::new(-3, 4);
    *params.term_mut(KING_PSQT_OFFSET + 24) = PhaseScore::new(5, -2);
    let engine = ClassicEngine::new(ClassicEngineConfig {
        eval: params,
        ..ClassicEngineConfig::default()
    });
    let mut rng = 0x243F_6A88_85A3_08D3u64;

    for index in 0..10_000 {
        let position = random_position(&mut rng);
        let game = Game::new(position);
        assert_eq!(
            engine.evaluate_score(&game),
            engine.evaluate(&game).side_to_move,
            "fast tapered score mismatch at sample {index}: {position:?}"
        );
    }
}

fn random_position(state: &mut u64) -> Position {
    let mut white = next_u32(state);
    while white.count_ones() > 12 {
        white &= white - 1;
    }
    let mut black = next_u32(state) & !white;
    while black.count_ones() > 12 {
        black &= black - 1;
    }
    if white | black == 0 {
        white = 1;
    }
    let kings = next_u32(state) & (white | black);
    let side_to_move = if next_u32(state) & 1 == 0 {
        Side::White
    } else {
        Side::Black
    };
    Position {
        white,
        black,
        kings,
        side_to_move,
    }
}

fn next_u32(state: &mut u64) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    (x >> 16) as u32
}

#[test]
fn extended_hce_is_color_symmetric_under_rotation() {
    let mut params = EvalParams::default();
    for (term, mg, eg) in [
        (20usize, -3, 2),
        (21, 7, 11),
        (22, 5, 3),
        (23, 4, 8),
        (24, 2, 6),
        (25, 9, 15),
        (26, 6, 12),
        (27, 8, 14),
    ] {
        *params.term_mut(term) = PhaseScore::new(mg, eg);
    }
    let engine = ClassicEngine::new(ClassicEngineConfig {
        eval: params,
        ..ClassicEngineConfig::default()
    });
    let p = Position {
        white: (1 << 0) | (1 << 5) | (1 << 10) | (1 << 18),
        black: (1 << 13) | (1 << 21) | (1 << 26) | (1 << 31),
        kings: (1 << 18) | (1 << 13),
        side_to_move: Side::White,
    };
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
