use crate::hash::{history_components, ZobristHash};
use crate::{apply_move, count_legal_moves, Move, Position, Side};

pub const MAX_GAME_PLIES: usize = 512;
const BIG_ROAD: u32 = (1u32 << 0)
    | (1u32 << 4)
    | (1u32 << 9)
    | (1u32 << 13)
    | (1u32 << 18)
    | (1u32 << 22)
    | (1u32 << 27)
    | (1u32 << 31);

/// Exact, collision-free identity of the board and side to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[repr(transparent)]
pub struct PositionKey(pub u128);

impl PositionKey {
    #[inline(always)]
    pub const fn from_position(pos: Position) -> Self {
        Self(
            (pos.white as u128)
                | ((pos.black as u128) << 32)
                | ((pos.kings as u128) << 64)
                | ((pos.side_to_move as u128) << 96),
        )
    }

    #[inline(always)]
    pub const fn side_to_move(self) -> Side {
        if ((self.0 >> 96) & 1) == 0 {
            Side::White
        } else {
            Side::Black
        }
    }

    #[inline(always)]
    pub const fn to_position(self) -> Position {
        Position {
            white: self.0 as u32,
            black: (self.0 >> 32) as u32,
            kings: (self.0 >> 64) as u32,
            side_to_move: self.side_to_move(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomaticDrawReason {
    MaterialUnchangedLimit,
    KingOnlyLimit,
    ThreeKingsVsLoneKingLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawClaim {
    ThreefoldRepetition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndgameAdjudication {
    LoneKingOnBigRoadFiveMoveRule,
    ObviousDrawFiveMoveRule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameResult {
    Win(Side),
    Draw(AutomaticDrawReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameStatus {
    Ongoing,
    ClaimableDraw(DrawClaim),
    NeedsAdjudication(EndgameAdjudication),
    Finished(GameResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuleCounters {
    /// Consecutive individual player moves with no capture and no promotion,
    /// while both sides have a king. The official rules define one "move" as
    /// one player's checker movement, so this is intentionally counted per ply.
    pub material_unchanged_plies: u8,
    /// Consecutive individual player moves where only a king moved and no
    /// capture occurred; likewise counted per ply by the rules' definition.
    pub king_only_plies: u8,
    /// Side currently subject to the 15-move 3+ kings vs lone king rule.
    pub three_vs_one_side: Option<Side>,
    /// Number of moves made by that side since the material relation arose.
    pub three_vs_one_moves: u8,
    /// Side currently subject to the 5-move lone-king-on-big-road rule.
    pub big_road_chaser: Option<Side>,
    /// Number of moves made by that side since the relation arose.
    pub big_road_chase_moves: u8,
}

#[derive(Clone)]
pub struct GameHistory {
    keys: [PositionKey; MAX_GAME_PLIES + 1],
    len: u16,
    reversible_start: u16,
    context_lo: u64,
    context_hi: u64,
}

impl GameHistory {
    pub fn new(initial: Position) -> Self {
        let mut keys = [PositionKey::default(); MAX_GAME_PLIES + 1];
        let initial_key = PositionKey::from_position(initial);
        keys[0] = initial_key;
        let (context_lo, context_hi) = history_components(initial_key.0);
        Self {
            keys,
            len: 1,
            reversible_start: 0,
            context_lo,
            context_hi,
        }
    }

    #[inline(always)]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    #[inline(always)]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    pub const fn remaining_capacity(&self) -> usize {
        (MAX_GAME_PLIES + 1) - self.len as usize
    }

    #[inline(always)]
    pub const fn reversible_start(&self) -> usize {
        self.reversible_start as usize
    }

    #[inline(always)]
    pub const fn reversible_len(&self) -> usize {
        self.len as usize - self.reversible_start as usize
    }

    #[inline(always)]
    pub fn current(&self) -> PositionKey {
        self.keys[self.len as usize - 1]
    }

    /// Exact key counted from the current position backwards. Offset 0 is the
    /// current position, 1 is the parent, and so on. This bounded accessor is
    /// useful for diagnostics and future evaluation/tablebase integrations
    /// without cloning the full 513-entry history buffer.
    #[inline(always)]
    pub fn key_from_end(&self, offset: usize) -> Option<PositionKey> {
        let len = self.len as usize;
        if offset >= len {
            None
        } else {
            Some(self.keys[len - 1 - offset])
        }
    }

    /// 128-bit order-independent fingerprint of every exact key in the current
    /// reversible window. It is used only to reject/accept TT candidates fast.
    /// Exact repetition claims still scan `keys`.
    #[inline(always)]
    pub const fn repetition_context(&self) -> u128 {
        (self.context_lo as u128) | ((self.context_hi as u128) << 64)
    }

    pub fn occurrences_of_current(&self) -> u8 {
        self.occurrences_of(self.current())
    }

    /// Count an exact position in the reversible window. Because `PositionKey`
    /// includes side-to-move, only every second history cell can match. This
    /// halves the repetition-scan work in the hot search path.
    pub fn occurrences_of(&self, key: PositionKey) -> u8 {
        let start = self.reversible_start as usize;
        let len = self.len as usize;
        if len == 0 {
            return 0;
        }

        let current_side = self.current().side_to_move();
        let mut i = if key.side_to_move() == current_side {
            len - 1
        } else {
            if len < 2 {
                return 0;
            }
            len - 2
        };

        let mut count = 0u8;
        loop {
            if i < start {
                break;
            }
            count += (self.keys[i] == key) as u8;
            if i < start + 2 {
                break;
            }
            i -= 2;
        }
        count
    }

    fn push(&mut self, pos: Position, reversible: bool) {
        let i = self.len as usize;
        assert!(i <= MAX_GAME_PLIES, "game history capacity exceeded");
        let key = PositionKey::from_position(pos);
        self.keys[i] = key;
        self.len += 1;

        let (lo, hi) = history_components(key.0);
        if reversible {
            self.context_lo = self.context_lo.wrapping_add(lo);
            self.context_hi = self.context_hi.wrapping_add(hi);
        } else {
            self.reversible_start = i as u16;
            self.context_lo = lo;
            self.context_hi = hi;
        }
    }
}

/// Compact undo record used by search. It restores one `Game::play` without
/// copying the fixed 513-entry history array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameUndo {
    position: Position,
    counters: RuleCounters,
    zobrist: ZobristHash,
    history_len: u16,
    reversible_start: u16,
    context_lo: u64,
    context_hi: u64,
}

impl GameUndo {
    pub const EMPTY: Self = Self {
        position: Position {
            white: 0,
            black: 0,
            kings: 0,
            side_to_move: Side::White,
        },
        counters: RuleCounters {
            material_unchanged_plies: 0,
            king_only_plies: 0,
            three_vs_one_side: None,
            three_vs_one_moves: 0,
            big_road_chaser: None,
            big_road_chase_moves: 0,
        },
        zobrist: ZobristHash(0),
        history_len: 0,
        reversible_start: 0,
        context_lo: 0,
        context_hi: 0,
    };
}

#[derive(Clone)]
pub struct Game {
    position: Position,
    counters: RuleCounters,
    history: GameHistory,
    /// Incrementally maintained; recomputing the whole board is unnecessary.
    zobrist: ZobristHash,
}

impl Game {
    pub fn new(position: Position) -> Self {
        let counters = RuleCounters {
            three_vs_one_side: three_vs_one_side(position),
            big_road_chaser: big_road_chaser(position),
            ..RuleCounters::default()
        };
        Self {
            position,
            counters,
            history: GameHistory::new(position),
            zobrist: ZobristHash::from_position(position),
        }
    }

    #[inline]
    pub fn initial() -> Self {
        Self::new(Position::initial())
    }

    #[inline(always)]
    pub const fn position(&self) -> Position {
        self.position
    }

    #[inline(always)]
    pub const fn counters(&self) -> RuleCounters {
        self.counters
    }

    #[inline(always)]
    pub const fn history(&self) -> &GameHistory {
        &self.history
    }

    #[inline(always)]
    pub const fn zobrist(&self) -> ZobristHash {
        self.zobrist
    }

    /// Play one move and return the minimal state needed to undo it.
    /// The large fixed history array is not copied.
    #[inline]
    pub fn play_with_undo(&mut self, mv: Move) -> GameUndo {
        let undo = GameUndo {
            position: self.position,
            counters: self.counters,
            zobrist: self.zobrist,
            history_len: self.history.len,
            reversible_start: self.history.reversible_start,
            context_lo: self.history.context_lo,
            context_hi: self.history.context_hi,
        };
        self.play(mv);
        undo
    }

    /// Restore a state returned by `play_with_undo`. Stale history cells above
    /// the restored length are intentionally left untouched and are invisible.
    #[inline]
    pub fn undo(&mut self, undo: GameUndo) {
        self.position = undo.position;
        self.counters = undo.counters;
        self.zobrist = undo.zobrist;
        self.history.len = undo.history_len;
        self.history.reversible_start = undo.reversible_start;
        self.history.context_lo = undo.context_lo;
        self.history.context_hi = undo.context_hi;
    }

    pub fn play(&mut self, mv: Move) {
        assert!(
            self.history.remaining_capacity() != 0,
            "game history capacity exceeded"
        );
        let parent = self.position;
        let mover = parent.side_to_move;
        let child_hash = self.zobrist.after_move(parent, mv);
        let child = apply_move(parent, mv);
        self.update_counters(parent, child, mv, mover);
        self.position = child;
        self.zobrist = child_hash;

        // Only a quiet king move is positionally reversible. A capture changes
        // material; a man move cannot be undone by a future quiet man move.
        let reversible = mv.moving_is_king() && !mv.is_capture();
        self.history.push(child, reversible);
    }

    #[cfg(test)]
    pub(crate) fn set_rule_counters_for_test(&mut self, counters: RuleCounters) {
        self.counters = counters;
    }

    #[cfg(test)]
    pub(crate) fn fill_history_to_capacity_for_test(&mut self) {
        let last = MAX_GAME_PLIES;
        let key = PositionKey::from_position(self.position);
        self.history.keys[last] = key;
        self.history.len = (MAX_GAME_PLIES + 1) as u16;
        self.history.reversible_start = last as u16;
        let (lo, hi) = history_components(key.0);
        self.history.context_lo = lo;
        self.history.context_hi = hi;
    }

    #[inline]
    pub fn packed_rule_state(&self) -> u64 {
        let three_side = option_side_code(self.counters.three_vs_one_side);
        let road_side = option_side_code(self.counters.big_road_chaser);
        let reversible_len = self.history.reversible_len() as u64;

        (self.counters.material_unchanged_plies as u64)
            | ((self.counters.king_only_plies as u64) << 8)
            | ((three_side as u64) << 16)
            | ((self.counters.three_vs_one_moves as u64) << 18)
            | ((road_side as u64) << 26)
            | ((self.counters.big_road_chase_moves as u64) << 28)
            | ((reversible_len & 0x03FF) << 36)
    }

    pub fn status(&self) -> GameStatus {
        self.status_with_legal_count(count_legal_moves(self.position))
    }

    /// Status checks that do not require generating legal moves. Search uses
    /// this before a TT probe so compatible TT cutoffs can avoid move
    /// generation without skipping automatic draws or external adjudication.
    /// `None` means that legal-move availability is still needed to distinguish
    /// an ordinary ongoing position from a loss by immobility.
    pub(crate) fn status_before_move_generation(&self) -> Option<GameStatus> {
        let ours = match self.position.side_to_move {
            Side::White => self.position.white,
            Side::Black => self.position.black,
        };
        if ours == 0 {
            return Some(GameStatus::Finished(GameResult::Win(
                self.position.side_to_move.opposite(),
            )));
        }

        if let Some(reason) = self.automatic_draw_reason() {
            return Some(GameStatus::Finished(GameResult::Draw(reason)));
        }

        if self.big_road_adjudication_due() {
            return Some(GameStatus::NeedsAdjudication(
                EndgameAdjudication::LoneKingOnBigRoadFiveMoveRule,
            ));
        }

        if self.history.occurrences_of_current() >= 3 {
            return Some(GameStatus::ClaimableDraw(DrawClaim::ThreefoldRepetition));
        }

        None
    }

    /// Variant for search code that has already generated legal moves. This
    /// avoids a second move-generation pass in the alpha-beta hot path.
    pub fn status_with_legal_count(&self, legal_count: usize) -> GameStatus {
        if legal_count == 0 {
            return GameStatus::Finished(GameResult::Win(self.position.side_to_move.opposite()));
        }
        self.status_before_move_generation()
            .unwrap_or(GameStatus::Ongoing)
    }

    pub fn automatic_draw_reason(&self) -> Option<AutomaticDrawReason> {
        let total = self.position.occupied().count_ones();
        let white_has_king = (self.position.white & self.position.kings) != 0;
        let black_has_king = (self.position.black & self.position.kings) != 0;

        if white_has_king && black_has_king {
            let threshold = match total {
                2 | 3 => Some(5),
                4 | 5 => Some(30),
                6 | 7 => Some(60),
                _ => None,
            };
            if let Some(limit) = threshold {
                if self.counters.material_unchanged_plies >= limit {
                    return Some(AutomaticDrawReason::MaterialUnchangedLimit);
                }
            }
        }

        let king_only_limit = match total {
            4 | 5 => Some(15),
            6 | 7 => Some(25),
            _ => None,
        };
        if let Some(limit) = king_only_limit {
            if self.counters.king_only_plies >= limit {
                return Some(AutomaticDrawReason::KingOnlyLimit);
            }
        }

        if self.counters.three_vs_one_side.is_some() && self.counters.three_vs_one_moves >= 15 {
            return Some(AutomaticDrawReason::ThreeKingsVsLoneKingLimit);
        }

        None
    }

    #[inline]
    pub fn threefold_claim_available(&self) -> bool {
        self.history.occurrences_of_current() >= 3
    }

    /// The rules also allow a player to claim before executing a legal move
    /// that would create the third occurrence. Only a quiet king move can be
    /// reversible in Russian draughts, so all other moves can return false
    /// without constructing a child history state.
    #[inline]
    pub fn move_creates_third_repetition(&self, mv: Move) -> bool {
        if !mv.moving_is_king() || mv.is_capture() {
            return false;
        }
        let child = apply_move(self.position, mv);
        self.history
            .occurrences_of(PositionKey::from_position(child))
            >= 2
    }

    fn big_road_adjudication_due(&self) -> bool {
        self.counters.big_road_chaser.is_some() && self.counters.big_road_chase_moves >= 5
    }

    fn update_counters(&mut self, _parent: Position, child: Position, mv: Move, mover: Side) {
        let child_white_kings = child.white & child.kings;
        let child_black_kings = child.black & child.kings;

        if !mv.is_capture() && !mv.promotes() && child_white_kings != 0 && child_black_kings != 0 {
            self.counters.material_unchanged_plies =
                self.counters.material_unchanged_plies.saturating_add(1);
        } else {
            self.counters.material_unchanged_plies = 0;
        }

        if mv.moving_is_king() && !mv.is_capture() {
            self.counters.king_only_plies = self.counters.king_only_plies.saturating_add(1);
        } else {
            self.counters.king_only_plies = 0;
        }

        self.update_three_vs_one(child, mover);
        self.update_big_road_chase(child, mover);
    }

    fn update_three_vs_one(&mut self, pos: Position, mover: Side) {
        let candidate = three_vs_one_side(pos);
        if candidate != self.counters.three_vs_one_side {
            self.counters.three_vs_one_side = candidate;
            self.counters.three_vs_one_moves = 0;
            return;
        }
        if candidate == Some(mover) {
            self.counters.three_vs_one_moves = self.counters.three_vs_one_moves.saturating_add(1);
        }
    }

    fn update_big_road_chase(&mut self, pos: Position, mover: Side) {
        let candidate = big_road_chaser(pos);
        if candidate != self.counters.big_road_chaser {
            self.counters.big_road_chaser = candidate;
            self.counters.big_road_chase_moves = 0;
            return;
        }
        if candidate == Some(mover) {
            self.counters.big_road_chase_moves =
                self.counters.big_road_chase_moves.saturating_add(1);
        }
    }
}

#[inline(always)]
const fn option_side_code(side: Option<Side>) -> u8 {
    match side {
        None => 0,
        Some(Side::White) => 1,
        Some(Side::Black) => 2,
    }
}

#[inline]
fn side_bits(pos: Position, side: Side) -> u32 {
    match side {
        Side::White => pos.white,
        Side::Black => pos.black,
    }
}

fn three_vs_one_side(pos: Position) -> Option<Side> {
    for side in [Side::White, Side::Black] {
        let ours = side_bits(pos, side);
        let enemy = side_bits(pos, side.opposite());
        let our_kings = ours & pos.kings;
        let enemy_kings = enemy & pos.kings;
        if our_kings.count_ones() >= 3 && enemy.count_ones() == 1 && enemy_kings == enemy {
            return Some(side);
        }
    }
    None
}

fn big_road_chaser(pos: Position) -> Option<Side> {
    for side in [Side::White, Side::Black] {
        let ours = side_bits(pos, side);
        let enemy = side_bits(pos, side.opposite());
        let enemy_kings = enemy & pos.kings;
        if ours.count_ones() == 3
            && (ours & BIG_ROAD) == 0
            && enemy.count_ones() == 1
            && enemy_kings == enemy
            && (enemy & BIG_ROAD) != 0
        {
            return Some(side);
        }
    }
    None
}
