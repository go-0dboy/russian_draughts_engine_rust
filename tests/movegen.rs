use russian_draughts_classic::{
    apply_move, coordinates, generate_moves, king_attacks_from, perft, square, Move, Position,
    Side, DEFAULT_MOVE_BUFFER_CAPACITY,
};

fn s(name: &str) -> u8 {
    let b = name.as_bytes();
    assert_eq!(b.len(), 2);
    square(b[0] - b'a', b[1] - b'1').unwrap_or_else(|| panic!("not a playable square: {name}"))
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

fn moves(pos: Position) -> ([Move; DEFAULT_MOVE_BUFFER_CAPACITY], usize) {
    let mut out = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = generate_moves(pos, &mut out);
    (out, len)
}

#[test]
fn square_mapping_round_trips_all_32_squares() {
    for sq in 0..32u8 {
        let (file, rank) = coordinates(sq).unwrap();
        assert_eq!(square(file, rank), Some(sq));
    }
    assert_eq!(s("a1"), 0);
    assert_eq!(s("c1"), 1);
    assert_eq!(s("b2"), 4);
    assert_eq!(s("h8"), 31);
    assert_eq!(square(1, 0), None);
}

#[test]
fn initial_position_has_seven_quiet_moves() {
    let (ms, len) = moves(Position::initial());
    assert_eq!(len, 7);
    assert!(ms[..len].iter().all(|m| !m.is_capture()));
}

#[test]
fn mandatory_capture_suppresses_all_quiet_moves() {
    let pos = position(&["a3", "e3"], &["b4"], &[], Side::White);
    let (ms, len) = moves(pos);
    assert_eq!(len, 1);
    assert!(ms[0].is_capture());
    assert_eq!(ms[0].from(), s("a3"));
    assert_eq!(ms[0].to(), s("c5"));
    assert_eq!(ms[0].capture_count(), 1);
}

#[test]
fn man_can_capture_backward() {
    let pos = position(&["c5"], &["b4"], &[], Side::White);
    let (ms, len) = moves(pos);
    assert_eq!(len, 1);
    assert_eq!(ms[0].from(), s("c5"));
    assert_eq!(ms[0].to(), s("a3"));
    assert_eq!(ms[0].captures(), bb(&["b4"]));
}

#[test]
fn complete_multi_capture_is_packed_into_one_u64() {
    let pos = position(&["a3"], &["b4", "d6"], &[], Side::White);
    let (ms, len) = moves(pos);
    assert_eq!(len, 1);
    let m = ms[0];
    assert_eq!(m.from(), s("a3"));
    assert_eq!(m.to(), s("e7"));
    assert_eq!(m.capture_count(), 2);
    assert_eq!(m.captures(), bb(&["b4", "d6"]));
    assert_eq!(m.raw() as u32, bb(&["b4", "d6"]));
    assert_ne!(m.raw(), 0);
}

#[test]
fn russian_rules_do_not_force_maximum_capture_count() {
    let pos = position(&["a3", "e3"], &["b4", "f4", "f6"], &[], Side::White);
    let (ms, len) = moves(pos);
    assert!(ms[..len].iter().all(|m| m.is_capture()));
    assert!(ms[..len]
        .iter()
        .any(|m| m.from() == s("a3") && m.capture_count() == 1));
    assert!(ms[..len]
        .iter()
        .any(|m| m.from() == s("e3") && m.capture_count() == 2));
}

#[test]
fn man_promotes_during_capture_and_continues_as_flying_king() {
    let pos = position(&["b6"], &["c7", "f6"], &[], Side::White);
    let (ms, len) = moves(pos);
    assert_eq!(len, 2);
    assert!(ms[..len]
        .iter()
        .all(|m| m.capture_count() == 2 && m.promotes()));
    assert!(ms[..len]
        .iter()
        .all(|m| !m.moving_is_king() && m.ends_as_king()));
    assert!(ms[..len].iter().all(|m| m.captures() == bb(&["c7", "f6"])));
    assert!(ms[..len].iter().any(|m| m.to() == s("g5")));
    assert!(ms[..len].iter().any(|m| m.to() == s("h4")));

    for &m in &ms[..len] {
        let next = apply_move(pos, m);
        assert_ne!(next.kings & (1u32 << m.to()), 0);
    }
}

#[test]
fn flying_king_has_all_quiet_ray_destinations() {
    let pos = position(&["c3"], &[], &["c3"], Side::White);
    let (ms, len) = moves(pos);
    assert_eq!(len, 11);
    assert!(ms[..len].iter().all(|m| !m.is_capture()));
    assert!(ms[..len]
        .iter()
        .all(|m| m.moving_is_king() && m.ends_as_king()));
    assert!(ms[..len].iter().all(|m| m.capture_count() == 0));
}

#[test]
fn flying_king_capture_offers_every_empty_landing_beyond_victim() {
    let pos = position(&["c3"], &["e5"], &["c3"], Side::White);
    let (ms, len) = moves(pos);
    assert_eq!(len, 3);
    assert!(ms[..len].iter().all(|m| m.capture_count() == 1));
    assert!(ms[..len].iter().all(|m| m.captures() == bb(&["e5"])));
    assert!(ms[..len].iter().any(|m| m.to() == s("f6")));
    assert!(ms[..len].iter().any(|m| m.to() == s("g7")));
    assert!(ms[..len].iter().any(|m| m.to() == s("h8")));
}

#[test]
fn already_captured_piece_remains_a_blocker_until_move_finishes() {
    let pos = position(&["c3"], &["d4", "b2"], &["c3"], Side::White);
    let (ms, len) = moves(pos);
    assert_eq!(len, 5);
    assert!(ms[..len].iter().all(|m| m.capture_count() == 1));
}

#[test]
fn own_piece_blocks_flying_king_capture_ray() {
    let pos = position(&["c3", "d4"], &["f6"], &["c3"], Side::White);
    let (ms, len) = moves(pos);
    assert!(ms[..len].iter().all(|m| !m.is_capture()));
}

#[test]
fn applying_capture_updates_side_pieces_and_kings() {
    let pos = position(&["c3"], &["e5"], &["c3", "e5"], Side::White);
    let (ms, len) = moves(pos);

    let mut selected = Move::EMPTY;
    for &m in &ms[..len] {
        if m.to() == s("h8") {
            selected = m;
            break;
        }
    }
    assert_ne!(selected, Move::EMPTY);

    let next = apply_move(pos, selected);
    assert_eq!(next.side_to_move, Side::Black);
    assert_eq!(next.white, bb(&["h8"]));
    assert_eq!(next.black, 0);
    assert_eq!(next.kings, bb(&["h8"]));
    assert!(next.is_valid());
}

fn slow_king_attacks(sq: u8, occupied: u32) -> u32 {
    let (file, rank) = coordinates(sq).unwrap();
    let dirs = [(1i8, 1i8), (-1, 1), (1, -1), (-1, -1)];
    let mut attacks = 0u32;

    for (df, dr) in dirs {
        let mut f = file as i8 + df;
        let mut r = rank as i8 + dr;
        while (0..8).contains(&f) && (0..8).contains(&r) {
            if let Some(to) = square(f as u8, r as u8) {
                let b = 1u32 << to;
                attacks |= b;
                if (occupied & b) != 0 {
                    break;
                }
            }
            f += df;
            r += dr;
        }
    }
    attacks
}

#[test]
fn magic_king_lookup_matches_reference_for_many_full_occupancies() {
    let mut x = 0x9E37_79B9u32;
    for sq in 0..32u8 {
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            assert_eq!(
                king_attacks_from(sq, x),
                slow_king_attacks(sq, x),
                "square={sq}, occupied={x:#010x}"
            );
        }
    }
}

#[test]
fn packed_move_caches_hot_path_metadata() {
    // a3xb4xc5xd6xe7: d6 is a king, b4 is a man.
    let pos = position(&["a3"], &["b4", "d6"], &["d6"], Side::White);
    let (ms, len) = moves(pos);
    assert_eq!(len, 1);
    let m = ms[0];

    assert!(m.is_capture());
    assert_eq!(m.capture_count(), 2);
    assert_eq!(m.captured_kings_count(), 1);
    assert!(!m.moving_is_king());
    assert!(!m.ends_as_king());
    assert!(!m.promotes());
    assert_eq!(m.from_to_key(), ((m.from() as u16) << 5) | m.to() as u16);

    // bit9 capture | bits5..8 capture_count | bits1..4 captured kings | bit0 promotion
    let expected_order_key = (1u16 << 9) | (2u16 << 5) | (1u16 << 1);
    assert_eq!(m.order_key(), expected_order_key);
    assert_eq!(m.capture_count(), m.captures().count_ones() as u8);
}

#[test]
fn cached_metadata_for_quiet_promotion_is_consistent() {
    let pos = position(&["b6"], &[], &[], Side::White);
    let (ms, len) = moves(pos);
    let promoted = ms[..len]
        .iter()
        .copied()
        .find(|m| m.to() == s("a7") || m.to() == s("c7"));

    // b6 -> a7/c7 is not the last rank; verify this setup does not falsely mark promotion.
    if let Some(m) = promoted {
        assert!(!m.promotes());
        assert!(!m.ends_as_king());
    }

    let pos = position(&["a7"], &[], &[], Side::White);
    let (ms, len) = moves(pos);
    assert_eq!(len, 1);
    let m = ms[0];
    assert_eq!(m.to(), s("b8"));
    assert!(m.promotes());
    assert!(!m.moving_is_king());
    assert!(m.ends_as_king());
    assert!(!m.is_capture());
    assert_eq!(m.capture_count(), 0);
    assert_eq!(m.captured_kings_count(), 0);
    assert_eq!(m.order_key(), 1);
}

#[test]
fn move_representation_is_exactly_64_bits() {
    assert_eq!(std::mem::size_of::<Move>(), 8);
}

#[test]
fn initial_position_is_valid() {
    assert!(Position::initial().is_valid());
}

#[test]
fn initial_position_perft_regression() {
    let pos = Position::initial();
    let expected = [
        1u64, 7, 49, 302, 1_469, 7_482, 37_986, 190_146, 929_901, 4_570_631, 22_449_922,
    ];

    let mut depth = 0u8;
    while (depth as usize) < expected.len() {
        assert_eq!(perft(pos, depth), expected[depth as usize], "depth={depth}");
        depth += 1;
    }
}
