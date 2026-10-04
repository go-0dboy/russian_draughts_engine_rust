use russian_draughts_classic::{
    count_legal_moves, generate_moves, Move, Position, Side, DEFAULT_MOVE_BUFFER_CAPACITY,
};

#[test]
fn exact_move_counter_matches_canonical_generator_and_internal_capacity_on_random_positions() {
    let mut state = 0x243F_6A88_85A3_08D3u64;
    let mut max_seen = 0usize;
    for sample in 0..100_000 {
        let position = random_position(&mut state);
        let counted = count_legal_moves(position);
        max_seen = max_seen.max(counted);
        assert!(
            counted <= DEFAULT_MOVE_BUFFER_CAPACITY,
            "internal move buffer too small at sample {sample}: count={counted} capacity={} position={position:?}",
            DEFAULT_MOVE_BUFFER_CAPACITY
        );
        let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let generated = generate_moves(position, &mut moves);
        assert_eq!(
            counted, generated,
            "move count mismatch at sample {sample}: {position:?}"
        );
    }
    eprintln!("max legal moves seen in deterministic stress sample: {max_seen}");
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
