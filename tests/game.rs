use russian_draughts_classic::{
    generate_moves, square, AutomaticDrawReason, Game, GameResult, GameStatus, Move, Position,
    PositionKey, Side, DEFAULT_MOVE_BUFFER_CAPACITY,
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

fn find_move(pos: Position, from: &str, to: &str) -> Move {
    let mut out = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = generate_moves(pos, &mut out);
    out[..len]
        .iter()
        .copied()
        .find(|m| m.from() == s(from) && m.to() == s(to))
        .unwrap_or_else(|| panic!("move {from}-{to} not found"))
}

#[test]
fn position_key_is_exact_and_side_sensitive() {
    let p = Position::initial();
    let mut q = p;
    q.side_to_move = Side::Black;
    assert_ne!(PositionKey::from_position(p), PositionKey::from_position(q));
    assert_eq!(PositionKey::from_position(p), PositionKey::from_position(p));
}

#[test]
fn no_pieces_is_loss_for_side_to_move() {
    let p = position(&["a1"], &[], &["a1"], Side::Black);
    let g = Game::new(p);
    assert_eq!(
        g.status(),
        GameStatus::Finished(GameResult::Win(Side::White))
    );
}

#[test]
fn exact_threefold_repetition_becomes_claimable() {
    // Four kings keep the repetition sequence below the 15/30-move automatic
    // thresholds, so this test exercises repetition rather than a terminal draw.
    let p = position(
        &["a1", "h8"],
        &["c1", "f8"],
        &["a1", "h8", "c1", "f8"],
        Side::White,
    );
    let mut g = Game::new(p);

    for _ in 0..2 {
        let m = find_move(g.position(), "a1", "c3");
        g.play(m);
        let m = find_move(g.position(), "c1", "e3");
        g.play(m);
        let m = find_move(g.position(), "c3", "a1");
        g.play(m);
        let m = find_move(g.position(), "e3", "c1");
        g.play(m);
    }

    assert!(g.threefold_claim_available());
    assert!(matches!(g.status(), GameStatus::ClaimableDraw(_)));
}

#[test]
fn irreversible_move_resets_repetition_window() {
    let p = position(&["a1", "c3"], &["h8"], &["a1", "h8"], Side::White);
    let mut g = Game::new(p);
    let old_base = g.history().reversible_start();

    // White man c3-d4: irreversible quiet man move.
    let m = find_move(g.position(), "c3", "d4");
    g.play(m);
    assert!(g.history().reversible_start() > old_base);
    assert_eq!(g.history().occurrences_of_current(), 1);
}

#[test]
fn rule_counters_are_updated_without_heap_state() {
    let p = position(&["a1"], &["h8"], &["a1", "h8"], Side::White);
    let mut g = Game::new(p);
    let m = find_move(g.position(), "a1", "b2");
    g.play(m);
    assert_eq!(g.counters().material_unchanged_plies, 1);
    assert_eq!(g.counters().king_only_plies, 1);
}

#[test]
fn two_piece_material_limit_triggers_on_fifth_individual_move_not_tenth_ply() {
    // The rules define one move as one player's checker movement. Therefore the
    // 5-move ending limit is reached after five plies, not after five full turns.
    let p = position(&["a1"], &["c1"], &["a1", "c1"], Side::White);
    let mut g = Game::new(p);
    let sequence = [
        ("a1", "c3"),
        ("c1", "e3"),
        ("c3", "a1"),
        ("e3", "c1"),
        ("a1", "c3"),
    ];

    for (i, (from, to)) in sequence.into_iter().enumerate() {
        let mv = find_move(g.position(), from, to);
        g.play(mv);
        if i < 4 {
            assert_eq!(g.automatic_draw_reason(), None);
        }
    }

    assert_eq!(g.counters().material_unchanged_plies, 5);
    assert_eq!(
        g.automatic_draw_reason(),
        Some(AutomaticDrawReason::MaterialUnchangedLimit)
    );
}

#[test]
fn four_piece_king_only_limit_triggers_exactly_on_fifteenth_move() {
    let p = position(
        &["a1", "h8"],
        &["c1", "f8"],
        &["a1", "h8", "c1", "f8"],
        Side::White,
    );
    let mut g = Game::new(p);
    let cycle = [("a1", "c3"), ("c1", "e3"), ("c3", "a1"), ("e3", "c1")];

    let mut ply = 0usize;
    while ply < 15 {
        let (from, to) = cycle[ply & 3];
        let mv = find_move(g.position(), from, to);
        g.play(mv);
        ply += 1;
        if ply < 15 {
            assert_ne!(
                g.automatic_draw_reason(),
                Some(AutomaticDrawReason::KingOnlyLimit)
            );
        }
    }

    assert_eq!(g.counters().king_only_plies, 15);
    assert_eq!(
        g.automatic_draw_reason(),
        Some(AutomaticDrawReason::KingOnlyLimit)
    );
}

#[test]
fn capture_and_promotion_reset_relevant_draw_counters() {
    // Capture reset.
    let p = position(&["a1", "c3"], &["h8", "e5"], &["a1", "h8"], Side::White);
    let mut g = Game::new(p);
    g.play(find_move(g.position(), "a1", "b2"));
    assert_eq!(g.counters().material_unchanged_plies, 1);
    g.play(find_move(g.position(), "e5", "d4"));
    assert_eq!(g.counters().material_unchanged_plies, 2);
    let capture = find_move(g.position(), "c3", "e5");
    assert!(capture.is_capture());
    g.play(capture);
    assert_eq!(g.counters().material_unchanged_plies, 0);
    assert_eq!(g.counters().king_only_plies, 0);

    // Promotion reset.
    let p = position(&["c1", "a7"], &["h8"], &["c1", "h8"], Side::White);
    let mut g = Game::new(p);
    g.play(find_move(g.position(), "c1", "a3"));
    g.play(find_move(g.position(), "h8", "f6"));
    assert_eq!(g.counters().material_unchanged_plies, 2);
    let promotion = find_move(g.position(), "a7", "b8");
    assert!(promotion.promotes());
    g.play(promotion);
    assert_eq!(g.counters().material_unchanged_plies, 0);
    assert_eq!(g.counters().king_only_plies, 0);
}

#[test]
fn history_capacity_failure_happens_before_game_state_mutation() {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    let p = position(
        &["a1", "h8"],
        &["c1", "f8"],
        &["a1", "h8", "c1", "f8"],
        Side::White,
    );
    let mut g = Game::new(p);
    let cycle = [("a1", "c3"), ("c1", "e3"), ("c3", "a1"), ("e3", "c1")];

    let mut i = 0usize;
    while g.history().remaining_capacity() != 0 {
        let (from, to) = cycle[i & 3];
        let mv = find_move(g.position(), from, to);
        g.play(mv);
        i += 1;
    }

    let before_position = g.position();
    let before_zobrist = g.zobrist();
    let before_counters = g.counters();
    let before_repetition = g.history().repetition_context();
    let before_len = g.history().len();
    let (from, to) = cycle[i & 3];
    let mv = find_move(g.position(), from, to);

    let result = catch_unwind(AssertUnwindSafe(|| g.play(mv)));
    assert!(result.is_err());
    assert_eq!(g.position(), before_position);
    assert_eq!(g.zobrist(), before_zobrist);
    assert_eq!(g.counters(), before_counters);
    assert_eq!(g.history().repetition_context(), before_repetition);
    assert_eq!(g.history().len(), before_len);
}
