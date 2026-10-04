#![deny(unsafe_code)]

//! High-performance classical engine for Russian draughts (draughts-64).
//!
//! The rules core uses u32 bitboards, packed u64 moves, caller-provided arrays,
//! table-driven men and multiplication-magic flying kings. The `classic`
//! module adds iterative deepening, negamax/PVS alpha-beta, a verified
//! transposition table, tactical quiescence and a tunable handcrafted
//! evaluation without any neural-network dependency.
//!
//! Move layout:
//! All 64 move bits are used: identity plus cached metadata for the hot search path.

pub mod api;
pub mod hash;
pub mod pdn;
mod tables;
pub mod training;
pub mod version;

pub use version::{ENGINE_ID, ENGINE_NAME, ENGINE_PACKAGE, ENGINE_VERSION};

use tables::{
    MAGICS, MAGIC_ATTACKS, MAGIC_MASKS, MAGIC_OFFSETS, MAGIC_SHIFTS, MAN_JUMP, MAN_QUIET, RAYS,
};

const WHITE_PROMOTION: u32 = 0xF000_0000;
const BLACK_PROMOTION: u32 = 0x0000_000F;
const TO_SHIFT: u32 = 32;
const FROM_SHIFT: u32 = 37;
const PROMOTES_SHIFT: u32 = 42;
const MOVING_KING_SHIFT: u32 = 43;
const ENDS_AS_KING_SHIFT: u32 = 44;
const IS_CAPTURE_SHIFT: u32 = 45;
const CAPTURE_COUNT_SHIFT: u32 = 46;
const CAPTURED_KINGS_COUNT_SHIFT: u32 = 50;
const ORDER_KEY_SHIFT: u32 = 54;

const SQUARE_MASK: u64 = 0x1F;
const NIBBLE_MASK: u64 = 0x0F;
const ORDER_KEY_MASK: u64 = 0x03FF;
const CAPTURES_MASK: u64 = 0xFFFF_FFFF;

pub const DEFAULT_MOVE_BUFFER_CAPACITY: usize = 256;

// Source-square masks for the four two-rank man capture directions on the
// compact 32-square board.  Rows are packed as four-bit nibbles.
const EVEN_RANKS: u32 = 0x0F0F_0F0F;
const ODD_RANKS: u32 = 0xF0F0_F0F0;
const MAN_CAP_PLUS_9: u32 = 0x0077_7777;
const MAN_CAP_PLUS_7: u32 = 0x00EE_EEEE;
const MAN_CAP_MINUS_7: u32 = 0x7777_7700;
const MAN_CAP_MINUS_9: u32 = 0xEEEE_EE00;

/// Return only men that can start at least one capture in the current root
/// position.  Continuations still use the exact DFS because Russian draughts
/// promotion/blocker semantics depend on the evolving capture sequence.
#[inline(always)]
fn men_with_initial_capture(our_men: u32, enemy: u32, occupied: u32) -> u32 {
    let empty = !occupied;

    // Landing +9: midpoint is +4 from even ranks and +5 from odd ranks.
    let plus_9 =
        MAN_CAP_PLUS_9 & (empty >> 9) & (((enemy >> 4) & EVEN_RANKS) | ((enemy >> 5) & ODD_RANKS));

    // Landing +7: midpoint is +3 from even ranks and +4 from odd ranks.
    let plus_7 =
        MAN_CAP_PLUS_7 & (empty >> 7) & (((enemy >> 3) & EVEN_RANKS) | ((enemy >> 4) & ODD_RANKS));

    // Landing -7: midpoint is -4 from even ranks and -3 from odd ranks.
    let minus_7 =
        MAN_CAP_MINUS_7 & (empty << 7) & (((enemy << 4) & EVEN_RANKS) | ((enemy << 3) & ODD_RANKS));

    // Landing -9: midpoint is -5 from even ranks and -4 from odd ranks.
    let minus_9 =
        MAN_CAP_MINUS_9 & (empty << 9) & (((enemy << 5) & EVEN_RANKS) | ((enemy << 4) & ODD_RANKS));

    our_men & (plus_9 | plus_7 | minus_7 | minus_9)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Side {
    White = 0,
    Black = 1,
}

impl Side {
    #[inline(always)]
    pub const fn opposite(self) -> Self {
        match self {
            Self::White => Self::Black,
            Self::Black => Self::White,
        }
    }

    #[inline(always)]
    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    pub white: u32,
    pub black: u32,
    pub kings: u32,
    pub side_to_move: Side,
}

impl Position {
    #[inline]
    pub const fn initial() -> Self {
        Self {
            white: 0x0000_0FFF,
            black: 0xFFF0_0000,
            kings: 0,
            side_to_move: Side::White,
        }
    }

    #[inline(always)]
    pub const fn occupied(self) -> u32 {
        self.white | self.black
    }

    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        (self.white & self.black) == 0
            && (self.kings & !self.occupied()) == 0
            && self.white.count_ones() <= 12
            && self.black.count_ones() <= 12
    }
}

/// Complete move packed into exactly 64 bits.
///
/// Layout:
/// - 0..31: bitboard of every captured piece;
/// - 32..36: destination square;
/// - 37..41: source square;
/// - 42: man became king during the move;
/// - 43: moving piece was already a king at move start;
/// - 44: moving piece is a king after the move;
/// - 45: capture flag;
/// - 46..49: cached capture count (0..12);
/// - 50..53: cached number of captured kings (0..12);
/// - 54..63: cached 10-bit static move-ordering key.
///
/// The upper 21 bits deliberately denormalize data already derivable from the
/// position/capture mask. They cost no extra storage and remove popcounts and
/// repeated position inspection from the search hot path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Move(u64);

impl Move {
    pub const EMPTY: Self = Self(0);

    #[inline(always)]
    const fn pack_capture(
        from: u8,
        to: u8,
        captures: u32,
        promotes: bool,
        moving_is_king: bool,
        capture_count: u8,
        captured_kings_count: u8,
    ) -> Self {
        debug_assert!(from < 32);
        debug_assert!(to < 32);
        debug_assert!(capture_count > 0 && capture_count <= 12);
        debug_assert!(captured_kings_count <= capture_count);

        // Captures always have IS_CAPTURE=1.  Building the key directly avoids
        // the generic constructor's branches in the hottest leaf path.
        let order_key = (1u16 << 9)
            | ((capture_count as u16) << 5)
            | ((captured_kings_count as u16) << 1)
            | (promotes as u16);
        let ends_as_king = moving_is_king || promotes;

        Self(
            (captures as u64)
                | ((to as u64) << TO_SHIFT)
                | ((from as u64) << FROM_SHIFT)
                | ((promotes as u64) << PROMOTES_SHIFT)
                | ((moving_is_king as u64) << MOVING_KING_SHIFT)
                | ((ends_as_king as u64) << ENDS_AS_KING_SHIFT)
                | (1u64 << IS_CAPTURE_SHIFT)
                | ((capture_count as u64) << CAPTURE_COUNT_SHIFT)
                | ((captured_kings_count as u64) << CAPTURED_KINGS_COUNT_SHIFT)
                | ((order_key as u64) << ORDER_KEY_SHIFT),
        )
    }

    #[inline(always)]
    const fn pack_quiet_man(from: u8, to: u8, promotes: bool) -> Self {
        debug_assert!(from < 32);
        debug_assert!(to < 32);

        // For a quiet man move every cached field is zero except promotion,
        // ends_as_king and order_key bit 0.
        let promotion = promotes as u64;
        Self(
            ((to as u64) << TO_SHIFT)
                | ((from as u64) << FROM_SHIFT)
                | (promotion << PROMOTES_SHIFT)
                | (promotion << ENDS_AS_KING_SHIFT)
                | (promotion << ORDER_KEY_SHIFT),
        )
    }

    #[inline(always)]
    const fn pack_quiet_king(from: u8, to: u8) -> Self {
        debug_assert!(from < 32);
        debug_assert!(to < 32);

        Self(
            ((to as u64) << TO_SHIFT)
                | ((from as u64) << FROM_SHIFT)
                | (1u64 << MOVING_KING_SHIFT)
                | (1u64 << ENDS_AS_KING_SHIFT),
        )
    }

    #[inline(always)]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Restore a stable packed move representation from raw bits.
    /// The bit layout itself constrains all encoded fields to their widths;
    /// callers loading untrusted data should additionally validate semantics.
    #[inline(always)]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    #[inline(always)]
    pub const fn from(self) -> u8 {
        ((self.0 >> FROM_SHIFT) & SQUARE_MASK) as u8
    }

    #[inline(always)]
    pub const fn to(self) -> u8 {
        ((self.0 >> TO_SHIFT) & SQUARE_MASK) as u8
    }

    /// Compact 10-bit `from * 32 + to` key obtained by one shift+mask.
    /// It is a coarse policy key: distinct multi-captures may share from/to.
    #[inline(always)]
    pub const fn from_to_key(self) -> u16 {
        ((self.0 >> TO_SHIFT) & ORDER_KEY_MASK) as u16
    }

    #[inline(always)]
    pub const fn captures(self) -> u32 {
        (self.0 & CAPTURES_MASK) as u32
    }

    /// Cached; does not execute a popcount.
    #[inline(always)]
    pub const fn capture_count(self) -> u8 {
        ((self.0 >> CAPTURE_COUNT_SHIFT) & NIBBLE_MASK) as u8
    }

    /// Cached number of captured kings; does not inspect the parent position.
    #[inline(always)]
    pub const fn captured_kings_count(self) -> u8 {
        ((self.0 >> CAPTURED_KINGS_COUNT_SHIFT) & NIBBLE_MASK) as u8
    }

    /// Cached single-bit test; does not test the 32-bit capture mask.
    #[inline(always)]
    pub const fn is_capture(self) -> bool {
        ((self.0 >> IS_CAPTURE_SHIFT) & 1) != 0
    }

    #[inline(always)]
    pub const fn promotes(self) -> bool {
        ((self.0 >> PROMOTES_SHIFT) & 1) != 0
    }

    /// True when the moving piece was a king before this move started.
    #[inline(always)]
    pub const fn moving_is_king(self) -> bool {
        ((self.0 >> MOVING_KING_SHIFT) & 1) != 0
    }

    /// True when the moving piece is a king in the child position.
    #[inline(always)]
    pub const fn ends_as_king(self) -> bool {
        ((self.0 >> ENDS_AS_KING_SHIFT) & 1) != 0
    }

    /// Precomputed 10-bit static move ordering key. Higher is searched first.
    #[inline(always)]
    pub const fn order_key(self) -> u16 {
        ((self.0 >> ORDER_KEY_SHIFT) & ORDER_KEY_MASK) as u16
    }
}

#[inline(always)]
pub const fn square(file: u8, rank: u8) -> Option<u8> {
    if file >= 8 || rank >= 8 || ((file + rank) & 1) != 0 {
        None
    } else {
        Some(rank * 4 + file / 2)
    }
}

#[inline(always)]
pub const fn coordinates(sq: u8) -> Option<(u8, u8)> {
    if sq >= 32 {
        return None;
    }
    let rank = sq / 4;
    let pos = sq & 3;
    let file = pos * 2 + (rank & 1);
    Some((file, rank))
}

#[inline(always)]
pub fn king_attacks_from(sq: u8, occupied: u32) -> u32 {
    debug_assert!(sq < 32);
    let s = sq as usize;
    let relevant = occupied & MAGIC_MASKS[s];
    let key = ((relevant as u64).wrapping_mul(MAGICS[s]) >> (MAGIC_SHIFTS[s] as u32)) as usize;
    MAGIC_ATTACKS[MAGIC_OFFSETS[s] as usize + key]
}

/// Count legal moves exactly. In the common quiet case this avoids materializing
/// packed moves: men use the precomputed quiet tables and kings use magic-ray
/// popcounts. If any capture exists we fall back to the canonical generator so
/// multi-capture path deduplication remains bit-for-bit identical.
pub fn count_legal_moves(pos: Position) -> usize {
    debug_assert!(pos.is_valid());
    let (ours, enemy) = match pos.side_to_move {
        Side::White => (pos.white, pos.black),
        Side::Black => (pos.black, pos.white),
    };
    let occupied = ours | enemy;
    let our_kings = ours & pos.kings;
    let our_men = ours & !our_kings;

    if men_with_initial_capture(our_men, enemy, occupied) != 0
        || kings_have_initial_capture(our_kings, enemy, occupied)
    {
        let mut scratch = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        return generate_moves(pos, &mut scratch);
    }

    let empty = !occupied;
    let mut count = 0usize;
    let mut men = our_men;
    while men != 0 {
        let sq = pop_lsb(&mut men);
        count += (MAN_QUIET[pos.side_to_move.index()][sq as usize] & empty).count_ones() as usize;
    }

    let mut kings = our_kings;
    while kings != 0 {
        let sq = pop_lsb(&mut kings);
        count += (king_attacks_from(sq, occupied) & empty).count_ones() as usize;
    }
    count
}

#[inline]
fn kings_have_initial_capture(mut kings: u32, enemy: u32, occupied: u32) -> bool {
    while kings != 0 {
        let sq = pop_lsb(&mut kings);
        let visible = king_attacks_from(sq, occupied);
        let blockers = visible & occupied;
        let mut dir = 0usize;
        while dir < 4 {
            let first = blockers & RAYS[sq as usize][dir];
            if first != 0 {
                let victim = first.trailing_zeros() as u8;
                let victim_bit = bit(victim);
                if enemy & victim_bit != 0 {
                    let landings = king_attacks_from(victim, occupied)
                        & RAYS[victim as usize][dir]
                        & !occupied;
                    if landings != 0 {
                        return true;
                    }
                }
            }
            dir += 1;
        }
    }
    false
}

/// Generate all legal moves into caller-provided storage.
/// Returns the number of valid entries in `out[..len]`.
/// No heap allocation is performed.
pub fn generate_moves(pos: Position, out: &mut [Move]) -> usize {
    debug_assert!(pos.is_valid());
    let mut len = 0usize;

    let (ours, enemy) = match pos.side_to_move {
        Side::White => (pos.white, pos.black),
        Side::Black => (pos.black, pos.white),
    };

    let our_kings = ours & pos.kings;
    let our_men = ours & !our_kings;
    let enemy_kings = enemy & pos.kings;

    // Most men in ordinary positions cannot start a capture.  Reject them
    // with four board-wide bit tests before entering the recursive DFS.
    let mut men = men_with_initial_capture(our_men, enemy, ours | enemy);
    while men != 0 {
        let sq = pop_lsb(&mut men);
        let dedup_start = len;
        capture_dfs(
            pos.side_to_move,
            sq,
            sq,
            ours,
            enemy,
            enemy_kings,
            0,
            0,
            0,
            false,
            false,
            dedup_start,
            out,
            &mut len,
        );
    }

    let mut kings = our_kings;
    while kings != 0 {
        let sq = pop_lsb(&mut kings);
        let dedup_start = len;
        capture_dfs(
            pos.side_to_move,
            sq,
            sq,
            ours,
            enemy,
            enemy_kings,
            0,
            0,
            0,
            true,
            false,
            dedup_start,
            out,
            &mut len,
        );
    }

    if len != 0 {
        return len;
    }

    generate_quiet_moves(
        pos.side_to_move,
        ours,
        our_men,
        our_kings,
        enemy,
        out,
        &mut len,
    );
    len
}

#[inline]
pub fn apply_move(mut pos: Position, mv: Move) -> Position {
    let from = mv.from();
    let to = mv.to();
    let captures = mv.captures();
    let from_bit = bit(from);
    let to_bit = bit(to);
    match pos.side_to_move {
        Side::White => {
            pos.white = (pos.white & !from_bit) | to_bit;
            pos.black &= !captures;
        }
        Side::Black => {
            pos.black = (pos.black & !from_bit) | to_bit;
            pos.white &= !captures;
        }
    }

    pos.kings &= !captures;
    pos.kings &= !from_bit;

    if mv.ends_as_king() {
        pos.kings |= to_bit;
    }

    pos.side_to_move = pos.side_to_move.opposite();
    pos
}

/// Canonical perft: geometrically different capture paths that produce the
/// identical child position count as one move.
pub fn perft(pos: Position, depth: u8) -> u64 {
    if depth == 0 {
        return 1;
    }

    // Allocate/zero one move buffer per recursion level once.  The old form
    // recreated a 2 KiB array at every node, which benchmarks the stack/zeroing
    // path as much as it benchmarks the move generator.
    let mut buffers = vec![[Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY]; depth as usize];
    perft_with_buffers(pos, depth, &mut buffers)
}

fn perft_with_buffers(
    pos: Position,
    depth: u8,
    buffers: &mut [[Move; DEFAULT_MOVE_BUFFER_CAPACITY]],
) -> u64 {
    if depth == 0 {
        return 1;
    }

    let (moves, child_buffers) = buffers
        .split_first_mut()
        .expect("perft buffer depth must match recursion depth");
    let len = generate_moves(pos, moves);

    if depth == 1 {
        return len as u64;
    }

    let mut nodes = 0u64;
    let mut i = 0usize;
    while i < len {
        // Copy the packed move before recursing so `moves` is not borrowed
        // across the child call.  `child_buffers` is disjoint by construction.
        let mv = moves[i];
        nodes = nodes.wrapping_add(perft_with_buffers(
            apply_move(pos, mv),
            depth - 1,
            child_buffers,
        ));
        i += 1;
    }
    nodes
}

#[allow(clippy::too_many_arguments)]
fn capture_dfs(
    side: Side,
    root: u8,
    sq: u8,
    ours: u32,
    enemy: u32,
    enemy_kings: u32,
    captured: u32,
    capture_count: u8,
    captured_kings_count: u8,
    is_king: bool,
    promoted: bool,
    dedup_start: usize,
    out: &mut [Move],
    len: &mut usize,
) {
    let occupied = ours | enemy;
    let mut expanded = false;

    if is_king {
        let visible = king_attacks_from(sq, occupied);
        let blockers = visible & occupied;
        let mut dir = 0usize;

        while dir < 4 {
            let first = blockers & RAYS[sq as usize][dir];
            if first != 0 {
                let victim = first.trailing_zeros() as u8;
                let victim_bit = bit(victim);

                if (enemy & victim_bit) != 0 && (captured & victim_bit) == 0 {
                    let mut landings = king_attacks_from(victim, occupied)
                        & RAYS[victim as usize][dir]
                        & !occupied;

                    while landings != 0 {
                        expanded = true;
                        let landing = pop_lsb(&mut landings);
                        let ours2 = move_piece(ours, sq, landing);

                        capture_dfs(
                            side,
                            root,
                            landing,
                            ours2,
                            enemy,
                            enemy_kings,
                            captured | victim_bit,
                            capture_count + 1,
                            captured_kings_count + ((enemy_kings & victim_bit != 0) as u8),
                            true,
                            promoted,
                            dedup_start,
                            out,
                            len,
                        );
                    }
                }
            }
            dir += 1;
        }
    } else {
        let mut dir = 0usize;
        while dir < 4 {
            let jump = MAN_JUMP[sq as usize][dir];
            if jump != 0xFFFF {
                let middle = (jump & 0x00FF) as u8;
                let landing = (jump >> 8) as u8;
                let middle_bit = bit(middle);

                if (enemy & middle_bit) != 0 && (captured & middle_bit) == 0 {
                    let landing_bit = bit(landing);
                    if (occupied & landing_bit) == 0 {
                        expanded = true;
                        let promoted_now = is_promotion_square(side, landing);
                        let ours2 = move_piece(ours, sq, landing);

                        capture_dfs(
                            side,
                            root,
                            landing,
                            ours2,
                            enemy,
                            enemy_kings,
                            captured | middle_bit,
                            capture_count + 1,
                            captured_kings_count + ((enemy_kings & middle_bit != 0) as u8),
                            promoted_now,
                            promoted || promoted_now,
                            dedup_start,
                            out,
                            len,
                        );
                    }
                }
            }
            dir += 1;
        }
    }

    if !expanded && captured != 0 {
        push_unique_from(
            out,
            len,
            dedup_start,
            Move::pack_capture(
                root,
                sq,
                captured,
                promoted,
                is_king && !promoted,
                capture_count,
                captured_kings_count,
            ),
        );
    }
}

#[inline]
fn generate_quiet_moves(
    side: Side,
    ours: u32,
    men: u32,
    kings: u32,
    enemy: u32,
    out: &mut [Move],
    len: &mut usize,
) {
    let occupied = ours | enemy;

    let mut pieces = men;
    while pieces != 0 {
        let from = pop_lsb(&mut pieces);
        let mut targets = MAN_QUIET[side.index()][from as usize] & !occupied;

        while targets != 0 {
            let to = pop_lsb(&mut targets);
            push(
                out,
                len,
                Move::pack_quiet_man(from, to, is_promotion_square(side, to)),
            );
        }
    }

    let mut pieces = kings;
    while pieces != 0 {
        let from = pop_lsb(&mut pieces);
        let mut targets = king_attacks_from(from, occupied) & !occupied;

        while targets != 0 {
            let to = pop_lsb(&mut targets);
            push(out, len, Move::pack_quiet_king(from, to));
        }
    }
}

#[inline(always)]
fn push(out: &mut [Move], len: &mut usize, mv: Move) {
    assert!(
        *len < out.len(),
        "move buffer too small: capacity={}",
        out.len()
    );
    out[*len] = mv;
    *len += 1;
}

#[inline]
fn push_unique_from(out: &mut [Move], len: &mut usize, start: usize, mv: Move) {
    // Capture results from different source pieces can never be identical
    // because `from` is packed into Move. Compare only against results emitted
    // by the current source piece instead of rescanning the whole move list.
    let raw = mv.raw();
    let mut i = start;
    while i < *len {
        if out[i].raw() == raw {
            return;
        }
        i += 1;
    }
    push(out, len, mv);
}

#[inline(always)]
const fn bit(sq: u8) -> u32 {
    1u32 << sq
}

#[inline(always)]
fn pop_lsb(bb: &mut u32) -> u8 {
    debug_assert!(*bb != 0);
    let sq = bb.trailing_zeros() as u8;
    *bb &= *bb - 1;
    sq
}

#[inline(always)]
const fn move_piece(board: u32, from: u8, to: u8) -> u32 {
    (board & !(1u32 << from)) | (1u32 << to)
}

#[inline(always)]
const fn is_promotion_square(side: Side, sq: u8) -> bool {
    let b = 1u32 << sq;
    match side {
        Side::White => (b & WHITE_PROMOTION) != 0,
        Side::Black => (b & BLACK_PROMOTION) != 0,
    }
}

pub mod classic;
pub mod game;

pub use classic::{
    benchmark_suite, eval_param_name, evaluation_phase, term_divisor, BenchmarkCase, ClassicEngine,
    ClassicEngineBuildError, ClassicEngineConfig, ClassicKey, ClassicSearchDecision, EvalBreakdown,
    EvalFeatures, EvalParams, Evaluator, PhaseScore, ReferenceSearchResult, ReferenceSearcher,
    SearchControl as ClassicSearchControl, SearchLimits, SearchParams,
    SearchResult as ClassicSearchResult, SearchStats as ClassicSearchStats,
    SearchStopReason as ClassicSearchStopReason, TranspositionTable, TtBound, TtHit,
    BASE_EVAL_TERM_COUNT, EVAL_FEATURE_COUNT, EVAL_PARAM_COUNT, EVAL_TERM_COUNT, KING_PSQT_OFFSET,
    MAN_PSQT_OFFSET, PHASE_SCALE, SEARCH_PARAM_NAMES, TT_WAYS,
};
pub use game::{
    AutomaticDrawReason, DrawClaim, EndgameAdjudication, Game, GameHistory, GameResult, GameStatus,
    GameUndo, PositionKey, RuleCounters, MAX_GAME_PLIES,
};
pub use hash::ZobristHash;
pub use pdn::{
    for_each_pdn_game, move_to_pdn, parse_fen, pdn_number_to_square, PdnError, PdnErrorKind,
    PdnGame, PdnMetadata, PdnOutcome, PdnStreamStats,
};
pub use training::{
    read_opening_file, read_tune_file, write_opening_file, write_tune_file, DataSplit, OpeningLine,
    TuneFileInfo, TuneRecord,
};
