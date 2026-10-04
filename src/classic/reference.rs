//! Small, deliberately simple reference search used to verify optimized
//! alpha-beta variants at shallow depths. It has no TT, no PVS, no LMR,
//! no aspiration windows and no selective extensions.

use crate::game::{GameResult, GameStatus};
use crate::{generate_moves, Game, Move, DEFAULT_MOVE_BUFFER_CAPACITY};

use super::eval::{EvalParams, Evaluator};
use super::search::{SCORE_INF, SCORE_WIN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceSearchResult {
    pub best_move: Option<Move>,
    pub score: i32,
    pub nodes: u64,
}

pub struct ReferenceSearcher {
    evaluator: Evaluator,
    qsearch_promotion_depth: u8,
    unresolved_adjudication_score: i32,
    nodes: u64,
}

impl ReferenceSearcher {
    pub fn new(eval: EvalParams) -> Self {
        Self {
            evaluator: Evaluator::new(eval),
            qsearch_promotion_depth: 2,
            unresolved_adjudication_score: 0,
            nodes: 0,
        }
    }

    pub fn search(&mut self, root: &Game, depth: u8) -> ReferenceSearchResult {
        self.nodes = 0;
        let mut game = root.clone();
        let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let len = generate_moves(game.position(), &mut moves);
        let status = game.status_with_legal_count(len);
        match status {
            GameStatus::Finished(result) => {
                return ReferenceSearchResult {
                    best_move: None,
                    score: terminal_score(game.position().side_to_move, result, 0),
                    nodes: self.nodes,
                };
            }
            GameStatus::NeedsAdjudication(_) => {
                return ReferenceSearchResult {
                    best_move: None,
                    score: self.unresolved_adjudication_score,
                    nodes: self.nodes,
                };
            }
            GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
        }

        let mut alpha = -SCORE_INF;
        let beta = SCORE_INF;
        let mut best = if matches!(status, GameStatus::ClaimableDraw(_)) {
            0
        } else {
            -SCORE_INF
        };
        let mut best_move = None;
        if best > alpha {
            alpha = best;
        }

        for &mv in &moves[..len] {
            let claim_by_move = game.move_creates_third_repetition(mv);
            let undo = game.play_with_undo(mv);
            let child = self.negamax(&mut game, depth.saturating_sub(1) as i16, 1, -beta, -alpha);
            game.undo(undo);
            let actual = -child;
            let score = if claim_by_move { actual.max(0) } else { actual };
            if score > best {
                best = score;
                best_move = Some(mv);
            }
            if score > alpha {
                alpha = score;
            }
        }

        ReferenceSearchResult {
            best_move,
            score: best,
            nodes: self.nodes,
        }
    }

    fn negamax(
        &mut self,
        game: &mut Game,
        depth: i16,
        ply: usize,
        mut alpha: i32,
        beta: i32,
    ) -> i32 {
        self.nodes = self.nodes.saturating_add(1);
        if depth <= 0 {
            return self.quiescence(game, ply, alpha, beta, self.qsearch_promotion_depth);
        }

        let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let len = generate_moves(game.position(), &mut moves);
        let status = game.status_with_legal_count(len);
        match status {
            GameStatus::Finished(result) => {
                return terminal_score(game.position().side_to_move, result, ply);
            }
            GameStatus::NeedsAdjudication(_) => return self.unresolved_adjudication_score,
            GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
        }

        let mut best = if matches!(status, GameStatus::ClaimableDraw(_)) {
            0
        } else {
            -SCORE_INF
        };
        if best > alpha {
            alpha = best;
        }
        if alpha >= beta {
            return best;
        }

        for &mv in &moves[..len] {
            let claim_by_move = game.move_creates_third_repetition(mv);
            let undo = game.play_with_undo(mv);
            let actual = -self.negamax(game, depth - 1, ply + 1, -beta, -alpha);
            game.undo(undo);
            let score = if claim_by_move { actual.max(0) } else { actual };
            if score > best {
                best = score;
            }
            if score > alpha {
                alpha = score;
            }
            if alpha >= beta {
                break;
            }
        }
        best
    }

    fn quiescence(
        &mut self,
        game: &mut Game,
        ply: usize,
        mut alpha: i32,
        beta: i32,
        promotion_depth: u8,
    ) -> i32 {
        self.nodes = self.nodes.saturating_add(1);
        let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let len = generate_moves(game.position(), &mut moves);
        let status = game.status_with_legal_count(len);
        match status {
            GameStatus::Finished(result) => {
                return terminal_score(game.position().side_to_move, result, ply);
            }
            GameStatus::NeedsAdjudication(_) => return self.unresolved_adjudication_score,
            GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
        }

        let has_capture = len > 0 && moves[0].is_capture();
        let current_claim = matches!(status, GameStatus::ClaimableDraw(_));
        let declared_draw_available = !has_capture
            && moves[..len]
                .iter()
                .copied()
                .any(|mv| game.move_creates_third_repetition(mv));
        let draw_available = current_claim || declared_draw_available;

        let mut best = if has_capture {
            if current_claim {
                0
            } else {
                -SCORE_INF
            }
        } else {
            let eval = self
                .evaluator
                .evaluate_with_current_moves(game, Some(&moves[..len]))
                .side_to_move;
            if draw_available {
                eval.max(0)
            } else {
                eval
            }
        };
        if best > alpha {
            alpha = best;
        }
        if alpha >= beta {
            return best;
        }

        if !has_capture && promotion_depth == 0 {
            return best;
        }

        for &mv in &moves[..len] {
            if !has_capture && !mv.promotes() {
                continue;
            }
            let claim_by_move = game.move_creates_third_repetition(mv);
            let undo = game.play_with_undo(mv);
            let next_promotion_depth = if !has_capture && mv.promotes() {
                promotion_depth.saturating_sub(1)
            } else {
                promotion_depth
            };
            let actual = -self.quiescence(game, ply + 1, -beta, -alpha, next_promotion_depth);
            game.undo(undo);
            let score = if claim_by_move { actual.max(0) } else { actual };
            if score > best {
                best = score;
            }
            if score > alpha {
                alpha = score;
            }
            if alpha >= beta {
                break;
            }
        }
        best
    }
}

impl Default for ReferenceSearcher {
    fn default() -> Self {
        Self::new(EvalParams::default())
    }
}

#[inline(always)]
fn terminal_score(side_to_move: crate::Side, result: GameResult, ply: usize) -> i32 {
    match result {
        GameResult::Win(winner) if winner == side_to_move => SCORE_WIN - ply as i32,
        GameResult::Win(_) => -SCORE_WIN + ply as i32,
        GameResult::Draw(_) => 0,
    }
}
