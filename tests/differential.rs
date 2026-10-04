use russian_draughts_classic::{
    coordinates, generate_moves, square, Move, Position, Side, DEFAULT_MOVE_BUFFER_CAPACITY,
};

const REF_CAPACITY: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    from: u8,
    to: u8,
    captures: u32,
    promotes: bool,
}

impl Key {
    const EMPTY: Self = Self {
        from: 0,
        to: 0,
        captures: 0,
        promotes: false,
    };
}

fn fast_key(m: Move) -> Key {
    Key {
        from: m.from(),
        to: m.to(),
        captures: m.captures(),
        promotes: m.promotes(),
    }
}

#[inline]
fn bit(sq: u8) -> u32 {
    1u32 << sq
}

fn step(sq: u8, df: i8, dr: i8) -> Option<u8> {
    let (file, rank) = coordinates(sq)?;
    let f = file as i8 + df;
    let r = rank as i8 + dr;
    if !(0..8).contains(&f) || !(0..8).contains(&r) {
        return None;
    }
    square(f as u8, r as u8)
}

fn is_promotion(side: Side, sq: u8) -> bool {
    match side {
        Side::White => sq >= 28,
        Side::Black => sq < 4,
    }
}

fn push_unique(out: &mut [Key; REF_CAPACITY], len: &mut usize, key: Key) {
    let mut i = 0usize;
    while i < *len {
        if out[i] == key {
            return;
        }
        i += 1;
    }
    assert!(*len < REF_CAPACITY);
    out[*len] = key;
    *len += 1;
}

#[allow(clippy::too_many_arguments)]
fn slow_capture_dfs(
    side: Side,
    root: u8,
    sq: u8,
    ours: u32,
    enemy: u32,
    captured: u32,
    is_king: bool,
    promoted: bool,
    out: &mut [Key; REF_CAPACITY],
    len: &mut usize,
) {
    const DIRS: [(i8, i8); 4] = [(1, 1), (-1, 1), (1, -1), (-1, -1)];
    let occupied = ours | enemy;
    let mut expanded = false;

    if is_king {
        for (df, dr) in DIRS {
            let mut cur = step(sq, df, dr);
            let mut victim = None;

            while let Some(t) = cur {
                let b = bit(t);
                if (occupied & b) != 0 {
                    victim = Some(t);
                    break;
                }
                cur = step(t, df, dr);
            }

            let Some(victim) = victim else {
                continue;
            };
            let victim_bit = bit(victim);
            if (enemy & victim_bit) == 0 || (captured & victim_bit) != 0 {
                continue;
            }

            let mut landing = step(victim, df, dr);
            while let Some(to) = landing {
                let to_bit = bit(to);
                if (occupied & to_bit) != 0 {
                    break;
                }

                expanded = true;
                let ours2 = (ours & !bit(sq)) | to_bit;
                slow_capture_dfs(
                    side,
                    root,
                    to,
                    ours2,
                    enemy,
                    captured | victim_bit,
                    true,
                    promoted,
                    out,
                    len,
                );
                landing = step(to, df, dr);
            }
        }
    } else {
        for (df, dr) in DIRS {
            let Some(middle) = step(sq, df, dr) else {
                continue;
            };
            let Some(to) = step(middle, df, dr) else {
                continue;
            };

            let middle_bit = bit(middle);
            let to_bit = bit(to);
            if (enemy & middle_bit) == 0 || (captured & middle_bit) != 0 || (occupied & to_bit) != 0
            {
                continue;
            }

            expanded = true;
            let promoted_now = is_promotion(side, to);
            let ours2 = (ours & !bit(sq)) | to_bit;
            slow_capture_dfs(
                side,
                root,
                to,
                ours2,
                enemy,
                captured | middle_bit,
                promoted_now,
                promoted || promoted_now,
                out,
                len,
            );
        }
    }

    if !expanded && captured != 0 {
        push_unique(
            out,
            len,
            Key {
                from: root,
                to: sq,
                captures: captured,
                promotes: promoted,
            },
        );
    }
}

fn slow_generate(pos: Position, out: &mut [Key; REF_CAPACITY]) -> usize {
    let (ours, enemy) = match pos.side_to_move {
        Side::White => (pos.white, pos.black),
        Side::Black => (pos.black, pos.white),
    };

    let kings = ours & pos.kings;
    let men = ours & !kings;
    let mut len = 0usize;

    let mut pieces = men;
    while pieces != 0 {
        let sq = pieces.trailing_zeros() as u8;
        pieces &= pieces - 1;
        slow_capture_dfs(
            pos.side_to_move,
            sq,
            sq,
            ours,
            enemy,
            0,
            false,
            false,
            out,
            &mut len,
        );
    }

    let mut pieces = kings;
    while pieces != 0 {
        let sq = pieces.trailing_zeros() as u8;
        pieces &= pieces - 1;
        slow_capture_dfs(
            pos.side_to_move,
            sq,
            sq,
            ours,
            enemy,
            0,
            true,
            false,
            out,
            &mut len,
        );
    }

    if len != 0 {
        return len;
    }

    let occupied = ours | enemy;
    let forward: [(i8, i8); 2] = match pos.side_to_move {
        Side::White => [(1, 1), (-1, 1)],
        Side::Black => [(1, -1), (-1, -1)],
    };

    let mut pieces = men;
    while pieces != 0 {
        let from = pieces.trailing_zeros() as u8;
        pieces &= pieces - 1;
        for (df, dr) in forward {
            let Some(to) = step(from, df, dr) else {
                continue;
            };
            if (occupied & bit(to)) == 0 {
                push_unique(
                    out,
                    &mut len,
                    Key {
                        from,
                        to,
                        captures: 0,
                        promotes: is_promotion(pos.side_to_move, to),
                    },
                );
            }
        }
    }

    const DIRS: [(i8, i8); 4] = [(1, 1), (-1, 1), (1, -1), (-1, -1)];
    let mut pieces = kings;
    while pieces != 0 {
        let from = pieces.trailing_zeros() as u8;
        pieces &= pieces - 1;
        for (df, dr) in DIRS {
            let mut cur = step(from, df, dr);
            while let Some(to) = cur {
                if (occupied & bit(to)) != 0 {
                    break;
                }
                push_unique(
                    out,
                    &mut len,
                    Key {
                        from,
                        to,
                        captures: 0,
                        promotes: false,
                    },
                );
                cur = step(to, df, dr);
            }
        }
    }

    len
}

fn next_u32(x: &mut u32) -> u32 {
    *x ^= *x << 13;
    *x ^= *x >> 17;
    *x ^= *x << 5;
    *x
}

fn random_position(seed: &mut u32) -> Position {
    let mut white = 0u32;
    let mut black = 0u32;
    let mut kings = 0u32;

    for sq in 0..32u8 {
        match next_u32(seed) % 7 {
            0 if white.count_ones() < 12 => white |= bit(sq),
            1 if black.count_ones() < 12 => black |= bit(sq),
            _ => {}
        }
    }

    let occupied = white | black;
    for sq in 0..32u8 {
        let b = bit(sq);
        if (occupied & b) != 0 && (next_u32(seed) & 3) == 0 {
            kings |= b;
        }
    }

    kings |= white & 0xF000_0000;
    kings |= black & 0x0000_000F;

    Position {
        white,
        black,
        kings,
        side_to_move: if (next_u32(seed) & 1) == 0 {
            Side::White
        } else {
            Side::Black
        },
    }
}

#[test]
fn optimized_generator_matches_independent_geometric_reference() {
    let mut seed = 0xA341_316Cu32;
    let mut fast = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let mut slow = [Key::EMPTY; REF_CAPACITY];
    let mut fast_keys = [Key::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let mut max_moves_seen = 0usize;

    for case in 0..10_000 {
        let pos = random_position(&mut seed);
        assert!(pos.is_valid());

        let fast_len = generate_moves(pos, &mut fast);
        let slow_len = slow_generate(pos, &mut slow);
        max_moves_seen = max_moves_seen.max(fast_len);

        assert_eq!(
            fast_len, slow_len,
            "move-count mismatch in case {case}: {pos:?}"
        );

        let mut i = 0usize;
        while i < fast_len {
            let m = fast[i];
            let capture_count = m.captures().count_ones() as u8;
            let captured_kings_count = (m.captures() & pos.kings).count_ones() as u8;
            let moving_is_king = (pos.kings & bit(m.from())) != 0;
            let ends_as_king = moving_is_king || m.promotes();
            let expected_order_key = ((m.is_capture() as u16) << 9)
                | ((capture_count as u16) << 5)
                | ((captured_kings_count as u16) << 1)
                | (m.promotes() as u16);

            assert_eq!(
                m.is_capture(),
                m.captures() != 0,
                "capture flag case {case}"
            );
            assert_eq!(
                m.capture_count(),
                capture_count,
                "capture count case {case}"
            );
            assert_eq!(
                m.captured_kings_count(),
                captured_kings_count,
                "captured kings case {case}"
            );
            assert_eq!(
                m.moving_is_king(),
                moving_is_king,
                "moving king case {case}"
            );
            assert_eq!(m.ends_as_king(), ends_as_king, "ends-as-king case {case}");
            assert_eq!(m.order_key(), expected_order_key, "order key case {case}");
            assert_eq!(
                m.from_to_key(),
                ((m.from() as u16) << 5) | m.to() as u16,
                "from/to key case {case}"
            );

            fast_keys[i] = fast_key(m);
            i += 1;
        }

        fast_keys[..fast_len].sort_unstable();
        slow[..slow_len].sort_unstable();

        assert_eq!(
            &fast_keys[..fast_len],
            &slow[..slow_len],
            "differential mismatch in case {case}: position={pos:?}"
        );
    }

    eprintln!("max generated moves seen in differential test: {max_moves_seen}");
}
