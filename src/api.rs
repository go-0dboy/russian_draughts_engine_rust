//! Checked product-facing API for untrusted positions and moves.
//!
//! The search core intentionally keeps trusted, allocation-free hot-path
//! operations. Applications should enter through this module so malformed
//! external data is rejected without panics or relying on packed `Move(u64)`
//! as a wire/storage format.

use std::fmt;

use crate::{count_legal_moves, generate_moves, Game, GameStatus, Move, Position, Side};

/// Stable application-facing move identity. Square indices are 0..31 in the
/// engine's documented playable-square numbering; `captures` is the exact set
/// of captured squares. Cached packed-move metadata is deliberately omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MoveSpec {
    pub from: u8,
    pub to: u8,
    pub captures: u32,
}

impl MoveSpec {
    #[inline(always)]
    pub const fn from_move(mv: Move) -> Self {
        Self {
            from: mv.from(),
            to: mv.to(),
            captures: mv.captures(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionValidationError {
    OverlappingSides,
    KingOutsideOccupiedSquares,
    TooManyWhitePieces,
    TooManyBlackPieces,
}

impl fmt::Display for PositionValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::OverlappingSides => "white and black bitboards overlap",
            Self::KingOutsideOccupiedSquares => "king bitboard contains an empty square",
            Self::TooManyWhitePieces => "white has more than 12 pieces",
            Self::TooManyBlackPieces => "black has more than 12 pieces",
        };
        f.write_str(text)
    }
}

impl std::error::Error for PositionValidationError {}

pub fn validate_position(position: Position) -> Result<(), PositionValidationError> {
    if position.white & position.black != 0 {
        return Err(PositionValidationError::OverlappingSides);
    }
    if position.kings & !position.occupied() != 0 {
        return Err(PositionValidationError::KingOutsideOccupiedSquares);
    }
    if position.white.count_ones() > 12 {
        return Err(PositionValidationError::TooManyWhitePieces);
    }
    if position.black.count_ones() > 12 {
        return Err(PositionValidationError::TooManyBlackPieces);
    }
    Ok(())
}

pub fn checked_position(
    white: u32,
    black: u32,
    kings: u32,
    side_to_move: Side,
) -> Result<Position, PositionValidationError> {
    let position = Position {
        white,
        black,
        kings,
        side_to_move,
    };
    validate_position(position)?;
    Ok(position)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveResolutionError {
    InvalidPosition(PositionValidationError),
    InvalidSquare,
    IllegalMove,
    AmbiguousMove,
    GameHistoryFull,
    GameFinished,
    AdjudicationRequired,
}

impl fmt::Display for MoveResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPosition(err) => write!(f, "invalid position: {err}"),
            Self::InvalidSquare => f.write_str("move contains a square outside 0..31"),
            Self::IllegalMove => f.write_str("move is not legal in the current position"),
            Self::AmbiguousMove => {
                f.write_str("move specification matches more than one legal move")
            }
            Self::GameHistoryFull => f.write_str("game history capacity exceeded"),
            Self::GameFinished => f.write_str("game is already finished"),
            Self::AdjudicationRequired => {
                f.write_str("game requires external adjudication before play can continue")
            }
        }
    }
}

impl std::error::Error for MoveResolutionError {}

impl From<PositionValidationError> for MoveResolutionError {
    fn from(value: PositionValidationError) -> Self {
        Self::InvalidPosition(value)
    }
}

/// Owned legal-move view for product code. It sizes storage from the exact
/// move counter before generation, so applications never depend on the core's
/// fixed caller-buffer convention.
pub struct LegalMoves {
    moves: Vec<Move>,
}

impl LegalMoves {
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.moves.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// Iterate over stable application-facing move identities. The packed
    /// engine representation remains private to this checked boundary.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = MoveSpec> + '_ {
        self.moves.iter().copied().map(MoveSpec::from_move)
    }

    #[inline(always)]
    fn packed_slice(&self) -> &[Move] {
        &self.moves
    }
}

pub fn legal_moves(position: Position) -> Result<LegalMoves, PositionValidationError> {
    validate_position(position)?;
    let count = count_legal_moves(position);
    let mut moves = vec![Move::EMPTY; count];
    let generated = generate_moves(position, &mut moves);
    debug_assert_eq!(generated, count);
    Ok(LegalMoves { moves })
}

/// Resolve a stable `MoveSpec` to the engine's packed hot-path `Move` by
/// matching against canonical legal moves. External callers never need to
/// construct cached metadata bits themselves.
pub fn resolve_legal_move(position: Position, spec: MoveSpec) -> Result<Move, MoveResolutionError> {
    validate_position(position)?;
    if spec.from >= 32 || spec.to >= 32 {
        return Err(MoveResolutionError::InvalidSquare);
    }
    let legal = legal_moves(position)?;
    let mut found = None;
    for &mv in legal.packed_slice() {
        if MoveSpec::from_move(mv) == spec {
            if found.is_some() {
                return Err(MoveResolutionError::AmbiguousMove);
            }
            found = Some(mv);
        }
    }
    found.ok_or(MoveResolutionError::IllegalMove)
}

/// Checked application-facing game wrapper. Search code may continue to use
/// `Game` directly; apps that receive external moves should use this wrapper.
#[derive(Clone)]
pub struct CheckedGame {
    inner: Game,
}

impl CheckedGame {
    pub fn new(position: Position) -> Result<Self, PositionValidationError> {
        validate_position(position)?;
        Ok(Self {
            inner: Game::new(position),
        })
    }

    #[inline]
    pub fn initial() -> Self {
        Self {
            inner: Game::initial(),
        }
    }

    #[inline(always)]
    pub const fn game(&self) -> &Game {
        &self.inner
    }

    #[inline(always)]
    pub const fn position(&self) -> Position {
        self.inner.position()
    }

    pub fn legal_moves(&self) -> Result<LegalMoves, PositionValidationError> {
        legal_moves(self.inner.position())
    }

    pub fn play(&mut self, spec: MoveSpec) -> Result<(), MoveResolutionError> {
        self.ensure_playable()?;
        let mv = resolve_legal_move(self.inner.position(), spec)?;
        self.inner.play(mv);
        Ok(())
    }

    pub fn play_packed_legacy(&mut self, raw: u64) -> Result<(), MoveResolutionError> {
        self.ensure_playable()?;
        validate_position(self.inner.position())?;
        let legal = legal_moves(self.inner.position())?;
        let mv = legal
            .packed_slice()
            .iter()
            .copied()
            .find(|mv| mv.raw() == raw)
            .ok_or(MoveResolutionError::IllegalMove)?;
        self.inner.play(mv);
        Ok(())
    }

    fn ensure_playable(&self) -> Result<(), MoveResolutionError> {
        match self.inner.status() {
            GameStatus::Finished(_) => return Err(MoveResolutionError::GameFinished),
            GameStatus::NeedsAdjudication(_) => {
                return Err(MoveResolutionError::AdjudicationRequired);
            }
            GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
        }
        if self.inner.history().remaining_capacity() == 0 {
            return Err(MoveResolutionError::GameHistoryFull);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AutomaticDrawReason, GameResult, RuleCounters};

    #[test]
    fn checked_game_rejects_play_after_automatic_draw_even_when_board_moves_exist() {
        // Build a legal two-king position with an already reached automatic
        // draw counter; board-legal moves still exist.
        let position = Position {
            white: 1u32 << 0,
            black: 1u32 << 31,
            kings: (1u32 << 0) | (1u32 << 31),
            side_to_move: Side::White,
        };
        let mut inner = Game::new(position);
        inner.set_rule_counters_for_test(RuleCounters {
            material_unchanged_plies: 5,
            king_only_plies: 5,
            ..RuleCounters::default()
        });
        assert!(matches!(
            inner.status(),
            GameStatus::Finished(GameResult::Draw(
                AutomaticDrawReason::MaterialUnchangedLimit
            )) | GameStatus::Finished(GameResult::Draw(AutomaticDrawReason::KingOnlyLimit))
        ));
        let mut checked = CheckedGame { inner };
        let spec = checked
            .legal_moves()
            .unwrap()
            .iter()
            .next()
            .expect("king must have a move");
        assert_eq!(checked.play(spec), Err(MoveResolutionError::GameFinished));
    }
    #[test]
    fn checked_game_rejects_play_when_external_adjudication_is_due() {
        let position = Position {
            white: 1u32 << 0,
            black: 1u32 << 31,
            kings: (1u32 << 0) | (1u32 << 31),
            side_to_move: Side::White,
        };
        let mut inner = Game::new(position);
        inner.set_rule_counters_for_test(RuleCounters {
            big_road_chaser: Some(Side::White),
            big_road_chase_moves: 5,
            ..RuleCounters::default()
        });
        assert!(matches!(inner.status(), GameStatus::NeedsAdjudication(_)));
        let mut checked = CheckedGame { inner };
        let spec = checked
            .legal_moves()
            .unwrap()
            .iter()
            .next()
            .expect("king must have a move");
        assert_eq!(
            checked.play(spec),
            Err(MoveResolutionError::AdjudicationRequired)
        );
    }
}
