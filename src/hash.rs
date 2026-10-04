use crate::{Move, Position, Side};

/// Incrementally maintained 64-bit Zobrist hash of board pieces and side to move.
///
/// This hash is an index/signature accelerator, never an authoritative equality
/// test. Exact board identity is provided by `PositionKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct ZobristHash(pub u64);

const PIECE_KIND_COUNT: usize = 4;
const SQUARE_COUNT: usize = 32;
const SEED: u64 = 0xD64F_3A91_7C25_B8E1;

const fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

const fn build_piece_keys() -> [[u64; SQUARE_COUNT]; PIECE_KIND_COUNT] {
    let mut out = [[0u64; SQUARE_COUNT]; PIECE_KIND_COUNT];
    let mut kind = 0usize;
    while kind < PIECE_KIND_COUNT {
        let mut sq = 0usize;
        while sq < SQUARE_COUNT {
            let serial = (kind * SQUARE_COUNT + sq) as u64;
            out[kind][sq] =
                splitmix64(SEED.wrapping_add(serial.wrapping_mul(0xD134_2543_DE82_EF95)));
            sq += 1;
        }
        kind += 1;
    }
    out
}

const PIECE_KEYS: [[u64; SQUARE_COUNT]; PIECE_KIND_COUNT] = build_piece_keys();
const SIDE_TO_MOVE_KEY: u64 = splitmix64(SEED ^ 0xA076_1D64_78BD_642F);

#[inline(always)]
const fn kind(side: Side, king: bool) -> usize {
    match (side, king) {
        (Side::White, false) => 0,
        (Side::White, true) => 1,
        (Side::Black, false) => 2,
        (Side::Black, true) => 3,
    }
}

#[inline(always)]
fn pop_lsb(bb: &mut u32) -> u8 {
    let sq = bb.trailing_zeros() as u8;
    *bb &= *bb - 1;
    sq
}

impl ZobristHash {
    #[inline]
    pub fn from_position(pos: Position) -> Self {
        let mut h = 0u64;

        let mut wm = pos.white & !pos.kings;
        while wm != 0 {
            let sq = pop_lsb(&mut wm);
            h ^= PIECE_KEYS[0][sq as usize];
        }

        let mut wk = pos.white & pos.kings;
        while wk != 0 {
            let sq = pop_lsb(&mut wk);
            h ^= PIECE_KEYS[1][sq as usize];
        }

        let mut bm = pos.black & !pos.kings;
        while bm != 0 {
            let sq = pop_lsb(&mut bm);
            h ^= PIECE_KEYS[2][sq as usize];
        }

        let mut bk = pos.black & pos.kings;
        while bk != 0 {
            let sq = pop_lsb(&mut bk);
            h ^= PIECE_KEYS[3][sq as usize];
        }

        if pos.side_to_move == Side::Black {
            h ^= SIDE_TO_MOVE_KEY;
        }

        Self(h)
    }

    /// Update a parent position hash using only the move and captured squares.
    /// Quiet moves are three XORs: remove source, add destination, toggle side.
    /// Captures add one XOR per captured piece.
    #[inline]
    pub fn after_move(self, parent: Position, mv: Move) -> Self {
        let mover = parent.side_to_move;
        let victim_side = mover.opposite();
        let from = mv.from() as usize;
        let to = mv.to() as usize;

        let mut h = self.0;
        h ^= PIECE_KEYS[kind(mover, mv.moving_is_king())][from];
        h ^= PIECE_KEYS[kind(mover, mv.ends_as_king())][to];

        let mut captured = mv.captures();
        while captured != 0 {
            let sq = pop_lsb(&mut captured);
            let was_king = (parent.kings & (1u32 << sq)) != 0;
            h ^= PIECE_KEYS[kind(victim_side, was_king)][sq as usize];
        }

        h ^= SIDE_TO_MOVE_KEY;
        Self(h)
    }
}

/// Strong avalanche mixer used for TT addressing and history fingerprints.
#[inline(always)]
pub(crate) const fn mix64(x: u64) -> u64 {
    splitmix64(x)
}

/// Two independent 64-bit components for the reversible-history multiset.
/// They are accelerators only; `GameHistory` remains the exact source of truth.
#[inline(always)]
pub(crate) const fn history_components(raw_key: u128) -> (u64, u64) {
    let lo = raw_key as u64;
    let hi = (raw_key >> 64) as u64;
    (
        splitmix64(lo ^ hi.rotate_left(17) ^ 0x243F_6A88_85A3_08D3),
        splitmix64(hi ^ lo.rotate_right(13) ^ 0x1319_8A2E_0370_7344),
    )
}
