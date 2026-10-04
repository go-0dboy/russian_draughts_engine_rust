use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::game::{EndgameAdjudication, GameResult, GameStatus};
use crate::{generate_moves, Game, Move, Side, DEFAULT_MOVE_BUFFER_CAPACITY};

use super::eval::{EvalBreakdown, EvalFeatures, EvalParams, Evaluator};
use super::search_params::SearchParams;
use super::tt::{Bound, ClassicKey, TranspositionTable};

pub const MAX_SEARCH_PLY: usize = 128;
pub const SCORE_INF: i32 = 32_000;
pub const SCORE_WIN: i32 = 30_000;
pub const SCORE_WIN_THRESHOLD: i32 = 29_000;
const MAX_HISTORY: i32 = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassicEngineConfig {
    pub tt_megabytes: usize,
    pub eval: EvalParams,
    /// Versioned strength parameters. The default is parsed from the embedded
    /// production `config/search_default.txt` and can be replaced by any strict
    /// `search_v1` candidate.
    pub search: SearchParams,
    /// Score used only when the official rules require an external
    /// adjudication/tablebase that is not built into the core rules module.
    /// Zero is deliberately conservative and is surfaced in search statistics.
    pub unresolved_adjudication_score: i32,
    /// Wall-clock checks are intentionally amortized. Node limits and external
    /// stop requests are still observed at every node.
    pub time_check_interval: u64,
}

impl Default for ClassicEngineConfig {
    fn default() -> Self {
        Self {
            tt_megabytes: 64,
            eval: EvalParams::default(),
            search: SearchParams::default(),
            unresolved_adjudication_score: 0,
            time_check_interval: 2_048,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchLimits {
    pub max_depth: u8,
    pub max_nodes: Option<u64>,
    pub max_time: Option<Duration>,
}

impl SearchLimits {
    pub const fn depth(max_depth: u8) -> Self {
        Self {
            max_depth,
            max_nodes: None,
            max_time: None,
        }
    }

    pub const fn nodes(max_depth: u8, max_nodes: u64) -> Self {
        Self {
            max_depth,
            max_nodes: Some(max_nodes),
            max_time: None,
        }
    }

    pub const fn timed(max_depth: u8, max_time: Duration) -> Self {
        Self {
            max_depth,
            max_nodes: None,
            max_time: Some(max_time),
        }
    }
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self::depth(12)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicSearchDecision {
    Move(Move),
    /// Current position can already be claimed as a threefold draw.
    ClaimDraw,
    /// The rules allow a claim before executing this declared move because it
    /// would create the third occurrence.
    ClaimDrawByMove(Move),
    Terminal(GameResult),
    NeedsAdjudication(EndgameAdjudication),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStopReason {
    Completed,
    TimeLimit,
    NodeLimit,
    ExternalStop,
    HistoryLimit,
    ProvenWinOrLoss,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// All alpha-beta and quiescence nodes.
    pub nodes: u64,
    /// Quiescence nodes; subset of `nodes`.
    pub qnodes: u64,
    pub eval_calls: u64,
    pub tt_probes: u64,
    pub tt_hits: u64,
    /// Same-position TT hits produced with a different extension budget. They
    /// remain move-ordering hints but their bounds are never trusted.
    pub tt_incompatible_extension_hits: u64,
    pub tt_cutoffs: u64,
    /// TT cutoffs completed before legal move generation.
    pub tt_early_cutoffs: u64,
    pub tt_stores: u64,
    pub beta_cutoffs: u64,
    pub first_move_cutoffs: u64,
    pub lmr_reductions: u64,
    pub lmr_researches: u64,
    pub pvs_researches: u64,
    pub aspiration_researches: u64,
    pub adjudication_nodes: u64,
    /// Should stay zero. Any non-zero value means a defensive PV legality
    /// validator had to truncate stale data before returning it to the caller.
    pub pv_truncations: u64,
    pub max_ply: u16,
    pub hashfull_per_mille: u16,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub decision: ClassicSearchDecision,
    /// Score from the root side-to-move point of view.
    pub score: i32,
    /// Last fully completed iterative-deepening depth.
    pub depth: u8,
    pub principal_variation: Vec<Move>,
    pub stop_reason: SearchStopReason,
    pub elapsed: Duration,
    pub stats: SearchStats,
}

impl SearchResult {
    pub fn best_move(&self) -> Option<Move> {
        match self.decision {
            ClassicSearchDecision::Move(mv) | ClassicSearchDecision::ClaimDrawByMove(mv) => {
                Some(mv)
            }
            _ => None,
        }
    }

    pub fn nodes_per_second(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            self.stats.nodes as f64 / secs
        } else {
            0.0
        }
    }
}

/// Cancellation handle bound to exactly one prepared search.
///
/// A handle never affects a later search. `ClassicEngine::control()` prepares
/// the next search and returns its handle, so a stop issued immediately after a
/// protocol `go` cannot be lost even if the worker thread has not entered
/// `search()` yet. Conversely, a late or repeated stop on a completed handle
/// cannot leak into the following search.
#[derive(Clone)]
pub struct SearchControl {
    stop_requested: Arc<AtomicBool>,
}

impl SearchControl {
    pub fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
    }

    #[inline(always)]
    fn is_stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }
}

struct SearchWorkspace {
    moves: Vec<[Move; DEFAULT_MOVE_BUFFER_CAPACITY]>,
    scores: Vec<[i32; DEFAULT_MOVE_BUFFER_CAPACITY]>,
}

impl SearchWorkspace {
    fn new() -> Self {
        Self {
            moves: vec![[Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY]; MAX_SEARCH_PLY],
            scores: vec![[0; DEFAULT_MOVE_BUFFER_CAPACITY]; MAX_SEARCH_PLY],
        }
    }
}

pub struct ClassicEngine {
    config: ClassicEngineConfig,
    evaluator: Evaluator,
    tt: TranspositionTable,
    history: [i32; 2 * 1024],
    killers: [[Move; 2]; MAX_SEARCH_PLY],
    counter_moves: [Move; 1024],
    pv: Vec<[Move; MAX_SEARCH_PLY]>,
    pv_len: [u8; MAX_SEARCH_PLY],
    prepared_stop: Option<SearchControl>,
    active_stop: SearchControl,
    stats: SearchStats,
    start: Instant,
    deadline: Option<Instant>,
    node_limit: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct RootIteration {
    decision: ClassicSearchDecision,
    score: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchAbort {
    TimeLimit,
    NodeLimit,
    ExternalStop,
    HistoryLimit,
}

impl SearchAbort {
    const fn stop_reason(self) -> SearchStopReason {
        match self {
            Self::TimeLimit => SearchStopReason::TimeLimit,
            Self::NodeLimit => SearchStopReason::NodeLimit,
            Self::ExternalStop => SearchStopReason::ExternalStop,
            Self::HistoryLimit => SearchStopReason::HistoryLimit,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassicEngineBuildError {
    InvalidSearchParams(String),
    ZeroTranspositionTableSize,
    ZeroTimeCheckInterval,
}

impl fmt::Display for ClassicEngineBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSearchParams(err) => write!(f, "invalid SearchParams: {err}"),
            Self::ZeroTranspositionTableSize => f.write_str("tt_megabytes must be > 0"),
            Self::ZeroTimeCheckInterval => f.write_str("time_check_interval must be > 0"),
        }
    }
}

impl std::error::Error for ClassicEngineBuildError {}

impl ClassicEngine {
    pub fn try_new(config: ClassicEngineConfig) -> Result<Self, ClassicEngineBuildError> {
        config
            .search
            .validate()
            .map_err(ClassicEngineBuildError::InvalidSearchParams)?;
        if config.tt_megabytes == 0 {
            return Err(ClassicEngineBuildError::ZeroTranspositionTableSize);
        }
        if config.time_check_interval == 0 {
            return Err(ClassicEngineBuildError::ZeroTimeCheckInterval);
        }
        let tt = TranspositionTable::new(config.tt_megabytes);
        Ok(Self {
            evaluator: Evaluator::new(config.eval),
            config,
            tt,
            history: [0; 2 * 1024],
            killers: [[Move::EMPTY; 2]; MAX_SEARCH_PLY],
            counter_moves: [Move::EMPTY; 1024],
            pv: vec![[Move::EMPTY; MAX_SEARCH_PLY]; MAX_SEARCH_PLY],
            pv_len: [0; MAX_SEARCH_PLY],
            prepared_stop: None,
            active_stop: SearchControl {
                stop_requested: Arc::new(AtomicBool::new(false)),
            },
            stats: SearchStats::default(),
            start: Instant::now(),
            deadline: None,
            node_limit: None,
        })
    }

    pub fn new(config: ClassicEngineConfig) -> Self {
        Self::try_new(config).unwrap_or_else(|err| panic!("{err}"))
    }

    pub fn config(&self) -> ClassicEngineConfig {
        self.config
    }

    /// Prepare the next search and return a cancellation handle scoped to that
    /// exact search. Calling this again before `search()` replaces the previous
    /// prepared handle; protocol adapters should prepare once per `go`.
    pub fn control(&mut self) -> SearchControl {
        let control = SearchControl {
            stop_requested: Arc::new(AtomicBool::new(false)),
        };
        self.prepared_stop = Some(control.clone());
        control
    }

    pub fn clear_transposition_table(&mut self) {
        self.tt.clear();
    }

    pub fn transposition_table_entries(&self) -> usize {
        self.tt.capacity_entries()
    }

    pub fn transposition_table_bytes(&self) -> usize {
        self.tt.allocated_bytes()
    }

    pub fn evaluate(&self, game: &Game) -> EvalBreakdown {
        self.evaluator.evaluate(game)
    }

    /// Fast score-only evaluation used by the search hot path.
    #[inline]
    pub fn evaluate_score(&self, game: &Game) -> i32 {
        self.evaluator.score(game)
    }

    pub fn eval_features(&self, game: &Game) -> EvalFeatures {
        self.evaluator.features(game)
    }

    pub fn search(&mut self, root: &Game, limits: SearchLimits) -> SearchResult {
        // Bind cancellation to this search. If a caller prepared a handle before
        // starting the worker, preserve it; otherwise create an unexposed token.
        // Every subsequent search receives a fresh token, so late stops cannot
        // cancel the next search.
        self.active_stop = self.prepared_stop.take().unwrap_or_else(|| SearchControl {
            stop_requested: Arc::new(AtomicBool::new(false)),
        });
        self.tt.new_search();
        self.history.fill(0);
        self.killers.fill([Move::EMPTY; 2]);
        self.counter_moves.fill(Move::EMPTY);
        self.pv_len.fill(0);
        self.stats = SearchStats::default();
        self.start = Instant::now();
        self.deadline = limits.max_time.and_then(|d| self.start.checked_add(d));
        self.node_limit = limits.max_nodes.filter(|&n| n > 0);

        let mut game = root.clone();
        let mut root_moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let root_len = generate_moves(game.position(), &mut root_moves);
        let root_status = game.status_with_legal_count(root_len);

        match root_status {
            GameStatus::Finished(result) => {
                return self.finish_result(
                    ClassicSearchDecision::Terminal(result),
                    terminal_score(game.position().side_to_move, result, 0),
                    0,
                    SearchStopReason::Completed,
                    Vec::new(),
                );
            }
            GameStatus::NeedsAdjudication(reason) => {
                return self.finish_result(
                    ClassicSearchDecision::NeedsAdjudication(reason),
                    self.config.unresolved_adjudication_score,
                    0,
                    SearchStopReason::Completed,
                    Vec::new(),
                );
            }
            GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
        }

        // Always have a legal fallback even if a very small time/node budget
        // stops before depth one completes. Preserve both forms of repetition
        // claim even in that emergency path.
        self.stats.eval_calls += 1;
        let root_static = self
            .evaluator
            .score_with_current_moves(&game, Some(&root_moves[..root_len]));
        let declared_draw = root_moves[..root_len]
            .iter()
            .copied()
            .find(|&mv| game.move_creates_third_repetition(mv));
        let (fallback_decision, fallback_score) =
            if matches!(root_status, GameStatus::ClaimableDraw(_)) {
                (ClassicSearchDecision::ClaimDraw, 0)
            } else if root_static <= 0 {
                if let Some(mv) = declared_draw {
                    (ClassicSearchDecision::ClaimDrawByMove(mv), 0)
                } else {
                    (ClassicSearchDecision::Move(root_moves[0]), root_static)
                }
            } else {
                (ClassicSearchDecision::Move(root_moves[0]), root_static)
            };

        let mut workspace = SearchWorkspace::new();
        let max_depth = limits.max_depth.max(1).min((MAX_SEARCH_PLY - 2) as u8);
        let mut best_decision = fallback_decision;
        let mut best_score = fallback_score;
        let mut best_depth = 0u8;
        let mut best_pv = Vec::new();
        let mut stop_reason = SearchStopReason::Completed;

        for depth in 1..=max_depth {
            let mut window = self.config.search.aspiration_window.max(8);
            let use_aspiration = depth >= self.config.search.aspiration_min_depth
                && best_depth > 0
                && best_score.abs() < SCORE_WIN_THRESHOLD;
            let mut alpha = if use_aspiration {
                (best_score - window).max(-SCORE_INF)
            } else {
                -SCORE_INF
            };
            let mut beta = if use_aspiration {
                (best_score + window).min(SCORE_INF)
            } else {
                SCORE_INF
            };

            let completed = loop {
                self.pv_len.fill(0);
                match self.root_search(
                    &mut game,
                    depth as i16,
                    alpha,
                    beta,
                    self.config.search.max_extensions_per_line,
                    &mut workspace,
                ) {
                    Ok(iteration) => {
                        if use_aspiration && iteration.score <= alpha && alpha > -SCORE_INF {
                            self.stats.aspiration_researches += 1;
                            window = (window * 2).min(SCORE_INF / 2);
                            alpha = (iteration.score - window).max(-SCORE_INF);
                            beta = (iteration.score + window).min(SCORE_INF);
                            continue;
                        }
                        if use_aspiration && iteration.score >= beta && beta < SCORE_INF {
                            self.stats.aspiration_researches += 1;
                            window = (window * 2).min(SCORE_INF / 2);
                            alpha = (iteration.score - window).max(-SCORE_INF);
                            beta = (iteration.score + window).min(SCORE_INF);
                            continue;
                        }
                        break Ok(iteration);
                    }
                    Err(abort) => break Err(abort),
                }
            };

            match completed {
                Ok(iteration) => {
                    best_decision = iteration.decision;
                    best_score = iteration.score;
                    best_depth = depth;
                    best_pv = match best_decision {
                        ClassicSearchDecision::Move(_) => {
                            let direct = self.pv_line(0);
                            self.validated_pv(root, &direct)
                        }
                        ClassicSearchDecision::ClaimDraw
                        | ClassicSearchDecision::ClaimDrawByMove(_)
                        | ClassicSearchDecision::Terminal(_)
                        | ClassicSearchDecision::NeedsAdjudication(_) => Vec::new(),
                    };
                    if best_score.abs() >= SCORE_WIN_THRESHOLD {
                        stop_reason = SearchStopReason::ProvenWinOrLoss;
                        break;
                    }
                }
                Err(abort) => {
                    stop_reason = abort.stop_reason();
                    break;
                }
            }
        }

        self.finish_result(best_decision, best_score, best_depth, stop_reason, best_pv)
    }

    fn finish_result(
        &mut self,
        decision: ClassicSearchDecision,
        score: i32,
        depth: u8,
        stop_reason: SearchStopReason,
        principal_variation: Vec<Move>,
    ) -> SearchResult {
        self.stats.hashfull_per_mille = self.tt.hashfull_per_mille();
        SearchResult {
            decision,
            score,
            depth,
            principal_variation,
            stop_reason,
            elapsed: self.start.elapsed(),
            stats: self.stats,
        }
    }

    fn root_search(
        &mut self,
        game: &mut Game,
        depth: i16,
        mut alpha: i32,
        beta: i32,
        extensions_left: u8,
        workspace: &mut SearchWorkspace,
    ) -> Result<RootIteration, SearchAbort> {
        self.visit_node(false, 0)?;
        if game.history().remaining_capacity() == 0 {
            return Err(SearchAbort::HistoryLimit);
        }
        self.pv_len[0] = 0;

        let len = generate_moves(game.position(), &mut workspace.moves[0]);
        let status = game.status_with_legal_count(len);
        match status {
            GameStatus::Finished(result) => {
                return Ok(RootIteration {
                    decision: ClassicSearchDecision::Terminal(result),
                    score: terminal_score(game.position().side_to_move, result, 0),
                });
            }
            GameStatus::NeedsAdjudication(reason) => {
                self.stats.adjudication_nodes += 1;
                return Ok(RootIteration {
                    decision: ClassicSearchDecision::NeedsAdjudication(reason),
                    score: self.config.unresolved_adjudication_score,
                });
            }
            GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
        }

        let key = ClassicKey::from_game(game);
        let hash = key.hash(game.zobrist().0);
        self.stats.tt_probes += 1;
        let tt_move = if let Some(hit) = self.tt.probe(key, hash, extensions_left) {
            self.stats.tt_hits += 1;
            if !hit.extension_compatible {
                self.stats.tt_incompatible_extension_hits += 1;
            }
            hit.best_move
        } else {
            Move::EMPTY
        };

        let side = game.position().side_to_move;
        self.order_moves(
            &mut workspace.moves[0][..len],
            &mut workspace.scores[0][..len],
            tt_move,
            0,
            side,
            Move::EMPTY,
        );

        let original_alpha = alpha;
        let mut best_score = -SCORE_INF;
        let mut best_move = Move::EMPTY;
        let mut best_decision = ClassicSearchDecision::Move(workspace.moves[0][0]);
        let current_claim = matches!(status, GameStatus::ClaimableDraw(_));
        if current_claim {
            best_score = 0;
            best_decision = ClassicSearchDecision::ClaimDraw;
            alpha = alpha.max(0);
            if alpha >= beta {
                return Ok(RootIteration {
                    decision: best_decision,
                    score: best_score,
                });
            }
        }

        for index in 0..len {
            let mv = workspace.moves[0][index];
            let claim_by_move = game.move_creates_third_repetition(mv);
            let undo = game.play_with_undo(mv);
            let extension = self.extension_for_move(len, depth, mv, extensions_left);
            let child_depth = depth - 1 + extension as i16;
            let child_extensions = extensions_left.saturating_sub(extension);

            let search_result = if index == 0 || !self.config.search.enable_pvs {
                self.negamax(
                    game,
                    child_depth,
                    1,
                    -beta,
                    -alpha,
                    true,
                    child_extensions,
                    mv,
                    workspace,
                )
                .map(|s| -s)
            } else {
                let probe = self
                    .negamax(
                        game,
                        child_depth,
                        1,
                        -alpha - 1,
                        -alpha,
                        false,
                        child_extensions,
                        mv,
                        workspace,
                    )
                    .map(|s| -s);
                match probe {
                    Ok(score) if score > alpha && score < beta => {
                        self.stats.pvs_researches += 1;
                        self.negamax(
                            game,
                            child_depth,
                            1,
                            -beta,
                            -alpha,
                            true,
                            child_extensions,
                            mv,
                            workspace,
                        )
                        .map(|s| -s)
                    }
                    other => other,
                }
            };
            game.undo(undo);
            let actual_score = search_result?;
            let score = if claim_by_move {
                actual_score.max(0)
            } else {
                actual_score
            };

            let prefer_declared_draw = !current_claim
                && claim_by_move
                && actual_score <= 0
                && score == best_score
                && !matches!(best_decision, ClassicSearchDecision::ClaimDrawByMove(_));
            if score > best_score || prefer_declared_draw {
                best_score = score;
                best_move = mv;
                if claim_by_move && actual_score <= 0 {
                    best_decision = ClassicSearchDecision::ClaimDrawByMove(mv);
                    self.pv_len[0] = 0;
                } else {
                    best_decision = ClassicSearchDecision::Move(mv);
                    self.set_pv(0, mv);
                }
            }

            if score > alpha {
                alpha = score;
            }
            if alpha >= beta {
                self.stats.beta_cutoffs += 1;
                if index == 0 {
                    self.stats.first_move_cutoffs += 1;
                }
                if !mv.is_capture() {
                    self.reward_quiet_cutoff(
                        side,
                        mv,
                        depth,
                        0,
                        Move::EMPTY,
                        &workspace.moves[0][..index],
                    );
                }
                break;
            }
        }

        let bound = classify_bound(best_score, original_alpha, beta);
        self.tt.store(
            key,
            hash,
            depth,
            extensions_left,
            score_to_tt(best_score, 0),
            bound,
            best_move,
        );
        self.stats.tt_stores += 1;

        Ok(RootIteration {
            decision: best_decision,
            score: best_score,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn negamax(
        &mut self,
        game: &mut Game,
        depth: i16,
        ply: usize,
        mut alpha: i32,
        mut beta: i32,
        pv_node: bool,
        extensions_left: u8,
        previous_move: Move,
        workspace: &mut SearchWorkspace,
    ) -> Result<i32, SearchAbort> {
        // PV state belongs to this exact node. Clear it before *any* early
        // return (qsearch, terminal, TT cutoff, depth guard) so a sibling can
        // never inherit stale continuation moves.
        if ply < MAX_SEARCH_PLY {
            self.pv_len[ply] = 0;
        }

        if depth <= 0 {
            return self.quiescence(
                game,
                ply,
                alpha,
                beta,
                self.config.search.qsearch_promotion_depth,
                previous_move,
                workspace,
            );
        }

        self.visit_node(false, ply)?;
        if game.history().remaining_capacity() == 0 {
            return Err(SearchAbort::HistoryLimit);
        }

        // Automatic draws and the external-adjudication state are known
        // without generating moves and must precede a TT cutoff. A no-legal-
        // move loss cannot have a TT entry produced by this engine because
        // terminal nodes are never stored, so a compatible TT hit also proves
        // that this exact search identity previously had legal moves.
        if let Some(status) = game.status_before_move_generation() {
            match status {
                GameStatus::Finished(GameResult::Win(winner)) => {
                    return Ok(terminal_score(
                        game.position().side_to_move,
                        GameResult::Win(winner),
                        ply,
                    ));
                }
                GameStatus::Finished(GameResult::Draw(_)) | GameStatus::NeedsAdjudication(_) => {
                    // Automatic draw/adjudication counters are known before
                    // move generation, but immobility has higher priority.
                    // These states are rare, so pay for one exact legal-move
                    // check here rather than changing the normal TT fast path.
                    let len = generate_moves(game.position(), &mut workspace.moves[ply]);
                    match game.status_with_legal_count(len) {
                        GameStatus::Finished(result) => {
                            return Ok(terminal_score(game.position().side_to_move, result, ply));
                        }
                        GameStatus::NeedsAdjudication(_) => {
                            self.stats.adjudication_nodes += 1;
                            return Ok(self.config.unresolved_adjudication_score);
                        }
                        GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
                    }
                }
                GameStatus::ClaimableDraw(_) | GameStatus::Ongoing => {}
            }
        }

        if ply >= MAX_SEARCH_PLY - 2 {
            let len = generate_moves(game.position(), &mut workspace.moves[ply]);
            let status = game.status_with_legal_count(len);
            match status {
                GameStatus::Finished(result) => {
                    return Ok(terminal_score(game.position().side_to_move, result, ply));
                }
                GameStatus::NeedsAdjudication(_) => {
                    self.stats.adjudication_nodes += 1;
                    return Ok(self.config.unresolved_adjudication_score);
                }
                GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
            }
            return Ok(self.static_eval(game, Some(&workspace.moves[ply][..len])));
        }

        // Mate-distance pruning keeps impossible longer wins/losses outside the
        // window and makes terminal distance stable across TT transpositions.
        alpha = alpha.max(-SCORE_WIN + ply as i32);
        beta = beta.min(SCORE_WIN - ply as i32 - 1);
        if alpha >= beta {
            return Ok(alpha);
        }

        let key = ClassicKey::from_game(game);
        let hash = key.hash(game.zobrist().0);
        self.stats.tt_probes += 1;
        let mut tt_move = Move::EMPTY;
        if let Some(hit) = self.tt.probe(key, hash, extensions_left) {
            self.stats.tt_hits += 1;
            tt_move = hit.best_move;
            if !hit.extension_compatible {
                self.stats.tt_incompatible_extension_hits += 1;
            } else {
                let tt_score = score_from_tt(hit.score, ply);
                if hit.depth >= depth {
                    let cutoff = match hit.bound {
                        Bound::Exact => true,
                        Bound::Lower => tt_score >= beta,
                        Bound::Upper => tt_score <= alpha,
                        Bound::None => false,
                    };
                    if cutoff {
                        self.stats.tt_cutoffs += 1;
                        self.stats.tt_early_cutoffs += 1;
                        return Ok(tt_score);
                    }
                }
            }
        }

        let len = generate_moves(game.position(), &mut workspace.moves[ply]);
        let status = game.status_with_legal_count(len);
        match status {
            GameStatus::Finished(result) => {
                return Ok(terminal_score(game.position().side_to_move, result, ply));
            }
            GameStatus::NeedsAdjudication(_) => {
                self.stats.adjudication_nodes += 1;
                return Ok(self.config.unresolved_adjudication_score);
            }
            GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
        }

        let side = game.position().side_to_move;
        self.order_moves(
            &mut workspace.moves[ply][..len],
            &mut workspace.scores[ply][..len],
            tt_move,
            ply,
            side,
            previous_move,
        );

        let original_alpha = alpha;
        let mut best_score = -SCORE_INF;
        let mut best_move = Move::EMPTY;

        let current_claim = matches!(status, GameStatus::ClaimableDraw(_));
        if current_claim {
            best_score = 0;
            alpha = alpha.max(0);
            if alpha >= beta {
                return Ok(0);
            }
        }

        for index in 0..len {
            let mv = workspace.moves[ply][index];
            let claim_by_move = game.move_creates_third_repetition(mv);
            let undo = game.play_with_undo(mv);
            let extension = self.extension_for_move(len, depth, mv, extensions_left);
            let full_child_depth = depth - 1 + extension as i16;
            let child_extensions = extensions_left.saturating_sub(extension);

            let mut reduction = 0i16;
            if self.config.search.enable_lmr
                && !pv_node
                && !mv.is_capture()
                && !mv.promotes()
                && !claim_by_move
                && depth >= self.config.search.lmr_min_depth
                && index + 1 >= self.config.search.lmr_min_move as usize
                && len > 1
            {
                reduction = lmr_reduction(self.config.search, depth, index);
                reduction = reduction.min((full_child_depth - 1).max(0));
            }

            let actual_score_result = if index == 0 || !self.config.search.enable_pvs {
                self.negamax(
                    game,
                    full_child_depth,
                    ply + 1,
                    -beta,
                    -alpha,
                    pv_node,
                    child_extensions,
                    mv,
                    workspace,
                )
                .map(|s| -s)
            } else {
                let reduced_depth = full_child_depth - reduction;
                if reduction > 0 {
                    self.stats.lmr_reductions += 1;
                }
                let first_probe = self
                    .negamax(
                        game,
                        reduced_depth,
                        ply + 1,
                        -alpha - 1,
                        -alpha,
                        false,
                        child_extensions,
                        mv,
                        workspace,
                    )
                    .map(|s| -s);

                match first_probe {
                    Ok(mut score) => {
                        if reduction > 0 && score > alpha {
                            self.stats.lmr_researches += 1;
                            match self.negamax(
                                game,
                                full_child_depth,
                                ply + 1,
                                -alpha - 1,
                                -alpha,
                                false,
                                child_extensions,
                                mv,
                                workspace,
                            ) {
                                Ok(s) => score = -s,
                                Err(abort) => {
                                    game.undo(undo);
                                    return Err(abort);
                                }
                            }
                        }
                        if score > alpha && score < beta {
                            self.stats.pvs_researches += 1;
                            match self.negamax(
                                game,
                                full_child_depth,
                                ply + 1,
                                -beta,
                                -alpha,
                                pv_node,
                                child_extensions,
                                mv,
                                workspace,
                            ) {
                                Ok(s) => score = -s,
                                Err(abort) => {
                                    game.undo(undo);
                                    return Err(abort);
                                }
                            }
                        }
                        Ok(score)
                    }
                    Err(abort) => Err(abort),
                }
            };

            game.undo(undo);
            let actual_score = actual_score_result?;
            let score = if claim_by_move {
                actual_score.max(0)
            } else {
                actual_score
            };

            let prefer_declared_draw = !current_claim
                && claim_by_move
                && actual_score <= 0
                && score == best_score
                && best_move != mv;
            if score > best_score || prefer_declared_draw {
                best_score = score;
                best_move = mv;
                if claim_by_move && actual_score <= 0 {
                    self.pv_len[ply] = 0;
                } else {
                    self.set_pv(ply, mv);
                }
            }
            if score > alpha {
                alpha = score;
            }

            if alpha >= beta {
                self.stats.beta_cutoffs += 1;
                if index == 0 {
                    self.stats.first_move_cutoffs += 1;
                }
                if !mv.is_capture() {
                    self.reward_quiet_cutoff(
                        side,
                        mv,
                        depth,
                        ply,
                        previous_move,
                        &workspace.moves[ply][..index],
                    );
                }
                break;
            }
        }

        let bound = classify_bound(best_score, original_alpha, beta);
        self.tt.store(
            key,
            hash,
            depth,
            extensions_left,
            score_to_tt(best_score, ply),
            bound,
            best_move,
        );
        self.stats.tt_stores += 1;
        Ok(best_score)
    }

    // Search state is intentionally passed as explicit scalars/references on this
    // hot recursive path; bundling them only to satisfy this style lint would add
    // an artificial wrapper with no ownership or semantic value.
    #[allow(clippy::too_many_arguments)]
    fn quiescence(
        &mut self,
        game: &mut Game,
        ply: usize,
        mut alpha: i32,
        beta: i32,
        promotion_depth: u8,
        previous_move: Move,
        workspace: &mut SearchWorkspace,
    ) -> Result<i32, SearchAbort> {
        self.visit_node(true, ply)?;
        if game.history().remaining_capacity() == 0 {
            return Err(SearchAbort::HistoryLimit);
        }

        let len = generate_moves(game.position(), &mut workspace.moves[ply]);
        let status = game.status_with_legal_count(len);
        match status {
            GameStatus::Finished(result) => {
                return Ok(terminal_score(game.position().side_to_move, result, ply));
            }
            GameStatus::NeedsAdjudication(_) => {
                self.stats.adjudication_nodes += 1;
                return Ok(self.config.unresolved_adjudication_score);
            }
            GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
        }
        if ply >= MAX_SEARCH_PLY - 2 {
            return Ok(self.static_eval(game, Some(&workspace.moves[ply][..len])));
        }

        let current_claim = matches!(status, GameStatus::ClaimableDraw(_));
        let has_capture = len > 0 && workspace.moves[ply][0].is_capture();
        // A player may also claim before executing a quiet king move that
        // would create the third occurrence. Even when qsearch does not search
        // ordinary quiet moves, that guaranteed draw must remain available.
        let declared_draw_available = !has_capture
            && workspace.moves[ply][..len]
                .iter()
                .copied()
                .any(|mv| game.move_creates_third_repetition(mv));
        let draw_available = current_claim || declared_draw_available;

        // Stand pat is legal only when there is no mandatory capture. Captures
        // are always searched until the position becomes quiet; each such move
        // removes material, so this cannot create an unbounded capture loop.
        let mut best_score = if has_capture {
            if current_claim {
                0
            } else {
                -SCORE_INF
            }
        } else {
            let eval = self.static_eval(game, Some(&workspace.moves[ply][..len]));
            if draw_available {
                eval.max(0)
            } else {
                eval
            }
        };

        if best_score >= beta {
            return Ok(best_score);
        }
        if best_score > alpha {
            alpha = best_score;
        }

        let search_promotions = !has_capture && promotion_depth > 0;
        if !has_capture && !search_promotions {
            return Ok(best_score);
        }

        let side = game.position().side_to_move;
        self.order_moves(
            &mut workspace.moves[ply][..len],
            &mut workspace.scores[ply][..len],
            Move::EMPTY,
            ply,
            side,
            previous_move,
        );

        for index in 0..len {
            let mv = workspace.moves[ply][index];
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
            let child = self.quiescence(
                game,
                ply + 1,
                -beta,
                -alpha,
                next_promotion_depth,
                mv,
                workspace,
            );
            game.undo(undo);
            let actual_score = -child?;
            let score = if claim_by_move {
                actual_score.max(0)
            } else {
                actual_score
            };

            if score > best_score {
                best_score = score;
            }
            if score > alpha {
                alpha = score;
            }
            if alpha >= beta {
                self.stats.beta_cutoffs += 1;
                if index == 0 {
                    self.stats.first_move_cutoffs += 1;
                }
                break;
            }
        }

        Ok(best_score)
    }

    fn static_eval(&mut self, game: &Game, current_moves: Option<&[Move]>) -> i32 {
        self.stats.eval_calls += 1;
        self.evaluator.score_with_current_moves(game, current_moves)
    }

    fn extension_for_move(
        &self,
        legal_count: usize,
        depth: i16,
        mv: Move,
        extensions_left: u8,
    ) -> u8 {
        if extensions_left == 0 {
            return 0;
        }
        if legal_count == 1 && depth >= self.config.search.forced_extension_min_depth {
            return 1;
        }
        // A quiet promotion is strategically discontinuous: the piece changes
        // from short-range to flying king. A conservative shallow extension
        // substantially reduces promotion horizon effects without exploding
        // ordinary middlegame trees.
        if mv.promotes()
            && !mv.is_capture()
            && depth <= self.config.search.promotion_extension_max_depth
        {
            return 1;
        }
        0
    }

    fn order_moves(
        &self,
        moves: &mut [Move],
        scores: &mut [i32],
        tt_move: Move,
        ply: usize,
        side: Side,
        previous_move: Move,
    ) {
        if moves.len() <= 1 {
            return;
        }

        // TT move is overwhelmingly likely to be searched first and has a
        // score above every other category. Move it to the front without
        // scoring or sorting the rest of the list around it.
        let mut start = 0usize;
        if tt_move != Move::EMPTY {
            if let Some(pos) = moves.iter().position(|&mv| mv == tt_move) {
                moves.swap(0, pos);
                start = 1;
            }
        }

        for i in start..moves.len() {
            scores[i] = self.move_order_score(moves[i], ply, side, previous_move);
        }

        // Legal move lists are short. Stable insertion sort has excellent
        // locality. The TT move, when present, is already fixed at index 0.
        let first_unsorted = if start == 0 { 1 } else { 2 };
        for i in first_unsorted..moves.len() {
            let mv = moves[i];
            let score = scores[i];
            let mut j = i;
            while j > start && scores[j - 1] < score {
                moves[j] = moves[j - 1];
                scores[j] = scores[j - 1];
                j -= 1;
            }
            moves[j] = mv;
            scores[j] = score;
        }
    }

    fn move_order_score(&self, mv: Move, ply: usize, side: Side, previous_move: Move) -> i32 {
        if mv.is_capture() {
            return 1_500_000
                + mv.order_key() as i32 * 512
                + mv.captured_kings_count() as i32 * 64
                + mv.capture_count() as i32 * 16;
        }
        if mv.promotes() {
            return 1_250_000 + self.history_score(side, mv);
        }
        if ply < MAX_SEARCH_PLY {
            if mv == self.killers[ply][0] {
                return 1_100_000;
            }
            if mv == self.killers[ply][1] {
                return 1_050_000;
            }
        }
        if previous_move != Move::EMPTY
            && mv == self.counter_moves[previous_move.from_to_key() as usize]
        {
            return 1_000_000;
        }
        self.history_score(side, mv)
    }

    fn reward_quiet_cutoff(
        &mut self,
        side: Side,
        mv: Move,
        depth: i16,
        ply: usize,
        previous_move: Move,
        previously_searched: &[Move],
    ) {
        if ply < MAX_SEARCH_PLY && self.killers[ply][0] != mv {
            self.killers[ply][1] = self.killers[ply][0];
            self.killers[ply][0] = mv;
        }
        if previous_move != Move::EMPTY {
            self.counter_moves[previous_move.from_to_key() as usize] = mv;
        }

        let bonus = history_bonus(self.config.search, depth);
        self.update_history(side, mv, bonus);
        let malus = -(bonus / self.config.search.history_malus_divisor).max(1);
        for &other in previously_searched {
            if !other.is_capture() && !other.promotes() {
                self.update_history(side, other, malus);
            }
        }
    }

    fn history_score(&self, side: Side, mv: Move) -> i32 {
        self.history[history_index(side, mv)]
    }

    fn update_history(&mut self, side: Side, mv: Move, bonus: i32) {
        let index = history_index(side, mv);
        let old = self.history[index];
        let bounded = bonus.clamp(-MAX_HISTORY, MAX_HISTORY);
        let gravity = old * bounded.abs() / MAX_HISTORY;
        self.history[index] = (old + bounded - gravity).clamp(-MAX_HISTORY, MAX_HISTORY);
    }

    fn set_pv(&mut self, ply: usize, mv: Move) {
        if ply >= MAX_SEARCH_PLY - 1 {
            return;
        }
        self.pv[ply][0] = mv;
        let child_len = self.pv_len[ply + 1] as usize;
        let max_copy = child_len.min(MAX_SEARCH_PLY - ply - 1);
        for i in 0..max_copy {
            let child_move = self.pv[ply + 1][i];
            self.pv[ply][i + 1] = child_move;
        }
        self.pv_len[ply] = (max_copy + 1) as u8;
    }

    fn pv_line(&self, ply: usize) -> Vec<Move> {
        let len = self.pv_len[ply] as usize;
        self.pv[ply][..len].to_vec()
    }

    fn validated_pv(&mut self, root: &Game, line: &[Move]) -> Vec<Move> {
        let mut game = root.clone();
        let mut out = Vec::with_capacity(line.len());
        for &mv in line {
            let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
            let len = generate_moves(game.position(), &mut legal);
            match game.status_with_legal_count(len) {
                GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
                GameStatus::Finished(_) | GameStatus::NeedsAdjudication(_) => break,
            }
            if !legal[..len].contains(&mv) || game.history().remaining_capacity() == 0 {
                self.stats.pv_truncations = self.stats.pv_truncations.saturating_add(1);
                break;
            }
            out.push(mv);
            game.play(mv);
        }
        out
    }

    fn visit_node(&mut self, qnode: bool, ply: usize) -> Result<(), SearchAbort> {
        self.stats.nodes = self.stats.nodes.saturating_add(1);
        if qnode {
            self.stats.qnodes = self.stats.qnodes.saturating_add(1);
        }
        self.stats.max_ply = self.stats.max_ply.max(ply.min(u16::MAX as usize) as u16);

        if self.active_stop.is_stop_requested() {
            return Err(SearchAbort::ExternalStop);
        }
        if let Some(limit) = self.node_limit {
            if self.stats.nodes >= limit {
                return Err(SearchAbort::NodeLimit);
            }
        }
        let interval = self.config.time_check_interval.max(1);
        if ply == 0 || self.stats.nodes % interval == 0 {
            if let Some(deadline) = self.deadline {
                if Instant::now() >= deadline {
                    return Err(SearchAbort::TimeLimit);
                }
            }
        }
        Ok(())
    }
}

impl Default for ClassicEngine {
    fn default() -> Self {
        Self::new(ClassicEngineConfig::default())
    }
}

#[inline(always)]
fn history_index(side: Side, mv: Move) -> usize {
    let side_index = match side {
        Side::White => 0usize,
        Side::Black => 1usize,
    };
    side_index * 1024 + mv.from_to_key() as usize
}

#[inline(always)]
fn history_bonus(params: SearchParams, depth: i16) -> i32 {
    let d = depth.max(1) as i32;
    (d * d * params.history_bonus_scale).min(params.history_bonus_max)
}

#[inline(always)]
fn lmr_reduction(params: SearchParams, depth: i16, move_index: usize) -> i16 {
    let move_number = move_index + 1;
    let mut reduction = params.lmr_base_reduction;
    if depth >= params.lmr_second_depth && move_number >= params.lmr_second_move as usize {
        reduction += params.lmr_second_reduction;
    }
    if depth >= params.lmr_third_depth && move_number >= params.lmr_third_move as usize {
        reduction += params.lmr_third_reduction;
    }
    reduction
}

#[inline(always)]
fn classify_bound(score: i32, original_alpha: i32, beta: i32) -> Bound {
    if score <= original_alpha {
        Bound::Upper
    } else if score >= beta {
        Bound::Lower
    } else {
        Bound::Exact
    }
}

#[inline(always)]
fn score_to_tt(score: i32, ply: usize) -> i32 {
    if score >= SCORE_WIN_THRESHOLD {
        score + ply as i32
    } else if score <= -SCORE_WIN_THRESHOLD {
        score - ply as i32
    } else {
        score
    }
}

#[inline(always)]
fn score_from_tt(score: i32, ply: usize) -> i32 {
    if score >= SCORE_WIN_THRESHOLD {
        score - ply as i32
    } else if score <= -SCORE_WIN_THRESHOLD {
        score + ply as i32
    } else {
        score
    }
}

#[inline(always)]
fn terminal_score(side_to_move: Side, result: GameResult, ply: usize) -> i32 {
    match result {
        GameResult::Win(winner) if winner == side_to_move => SCORE_WIN - ply as i32,
        GameResult::Win(_) => -SCORE_WIN + ply as i32,
        GameResult::Draw(_) => 0,
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::{generate_moves, square, DEFAULT_MOVE_BUFFER_CAPACITY};

    fn find_move(game: &Game, from: &str, to: &str) -> Move {
        let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let len = generate_moves(game.position(), &mut moves);
        let fb = from.as_bytes();
        let tb = to.as_bytes();
        let from_sq = square(fb[0] - b'a', fb[1] - b'1').expect("playable from square");
        let to_sq = square(tb[0] - b'a', tb[1] - b'1').expect("playable to square");
        moves[..len]
            .iter()
            .copied()
            .find(|mv| mv.from() == from_sq && mv.to() == to_sq)
            .expect("diagnostic move must be legal")
    }

    #[test]
    fn tt_score_from_different_extension_budget_cannot_replace_current_search() {
        let mut game = Game::initial();
        let opening = find_move(&game, "c3", "b4");
        game.play(opening);

        let search = SearchParams {
            enable_pvs: false,
            enable_lmr: false,
            aspiration_window: SCORE_INF,
            ..SearchParams::default()
        };
        let config = ClassicEngineConfig {
            search,
            ..ClassicEngineConfig::default()
        };

        let mut fresh = ClassicEngine::new(config);
        fresh.tt.new_search();
        let mut fresh_workspace = SearchWorkspace::new();
        let fresh_score = fresh
            .root_search(
                &mut game.clone(),
                3,
                -SCORE_INF,
                SCORE_INF,
                2,
                &mut fresh_workspace,
            )
            .expect("fresh search must complete")
            .score;

        let mut warmed = ClassicEngine::new(config);
        warmed.tt.new_search();
        let mut warm_workspace = SearchWorkspace::new();
        let zero_extension_score = warmed
            .root_search(
                &mut game.clone(),
                3,
                -SCORE_INF,
                SCORE_INF,
                0,
                &mut warm_workspace,
            )
            .expect("warm-up search must complete")
            .score;
        assert_ne!(
            zero_extension_score, fresh_score,
            "diagnostic position must distinguish the two extension budgets"
        );

        warmed.stats = SearchStats::default();
        warmed.pv_len.fill(0);
        let warmed_score = warmed
            .root_search(
                &mut game.clone(),
                3,
                -SCORE_INF,
                SCORE_INF,
                2,
                &mut warm_workspace,
            )
            .expect("compatible search must complete")
            .score;

        assert!(
            warmed.stats.tt_incompatible_extension_hits > 0,
            "warm TT should expose at least the root entry as an incompatible ordering hint"
        );
        assert_eq!(
            warmed_score, fresh_score,
            "TT entry searched with another extension budget changed the result"
        );
    }
    #[test]
    fn compatible_tt_entry_can_cut_off_before_move_generation() {
        let mut game = Game::initial();
        let mut engine = ClassicEngine::default();
        engine.tt.new_search();
        let extensions = engine.config.search.max_extensions_per_line;
        let mut workspace = SearchWorkspace::new();

        let first = engine
            .root_search(
                &mut game,
                4,
                -SCORE_INF,
                SCORE_INF,
                extensions,
                &mut workspace,
            )
            .expect("first search must complete");

        engine.stats = SearchStats::default();
        engine.pv_len.fill(0);
        let second = engine
            .root_search(
                &mut game,
                4,
                -SCORE_INF,
                SCORE_INF,
                extensions,
                &mut workspace,
            )
            .expect("second search must complete");

        assert_eq!(second.score, first.score);
        assert!(
            engine.stats.tt_early_cutoffs > 0,
            "second identical search should reuse compatible TT bounds before move generation"
        );
    }

    #[test]
    fn prepared_stop_is_not_lost_and_never_leaks_to_next_search() {
        let game = Game::initial();
        let config = ClassicEngineConfig {
            time_check_interval: 1,
            ..ClassicEngineConfig::default()
        };
        let mut engine = ClassicEngine::new(config);
        let control = engine.control();

        control.request_stop();
        let stopped = engine.search(&game, SearchLimits::depth(64));
        assert_eq!(stopped.stop_reason, SearchStopReason::ExternalStop);
        assert_eq!(stopped.depth, 0);
        assert!(stopped.best_move().is_some());

        // Repeated/late stop on the completed search's token stays scoped to it.
        control.request_stop();
        let next = engine.search(&game, SearchLimits::nodes(4, 10_000));
        assert_ne!(next.stop_reason, SearchStopReason::ExternalStop);
    }

    #[test]
    fn late_stop_after_natural_completion_does_not_cancel_next_search() {
        let game = Game::initial();
        let mut engine = ClassicEngine::default();
        let old = engine.control();
        let completed = engine.search(&game, SearchLimits::nodes(2, 10_000));
        assert_ne!(completed.stop_reason, SearchStopReason::ExternalStop);
        old.request_stop();
        old.request_stop();
        let next = engine.search(&game, SearchLimits::nodes(2, 10_000));
        assert_ne!(next.stop_reason, SearchStopReason::ExternalStop);
    }

    #[test]
    fn stop_on_completed_terminal_search_does_not_leak() {
        let terminal = Game::new(crate::Position {
            white: 0,
            black: 1,
            kings: 0,
            side_to_move: Side::White,
        });
        let mut engine = ClassicEngine::default();
        let old = engine.control();
        let result = engine.search(&terminal, SearchLimits::depth(8));
        assert!(matches!(
            result.decision,
            ClassicSearchDecision::Terminal(_)
        ));
        old.request_stop();
        let next = engine.search(&Game::initial(), SearchLimits::nodes(2, 10_000));
        assert_ne!(next.stop_reason, SearchStopReason::ExternalStop);
    }
    #[test]
    fn search_near_history_capacity_returns_safe_fallback_instead_of_panicking() {
        let mut game = Game::initial();
        game.fill_history_to_capacity_for_test();
        let mut engine = ClassicEngine::default();
        let result = engine.search(&game, SearchLimits::depth(8));
        assert_eq!(result.stop_reason, SearchStopReason::HistoryLimit);
        assert_eq!(result.depth, 0);
        assert!(result.best_move().is_some());
    }
    #[test]
    fn immobility_precedes_automatic_draw_in_negamax() {
        let sq = |file: u8, rank: u8| 1u32 << square(file, rank).expect("playable square");
        let pos = crate::Position {
            white: sq(1, 7),
            black: sq(3, 7) | sq(5, 7) | sq(7, 7),
            kings: 0,
            side_to_move: Side::White,
        };
        let mut game = Game::new(pos);
        game.set_rule_counters_for_test(crate::RuleCounters {
            king_only_plies: 15,
            ..crate::RuleCounters::default()
        });

        let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        assert_eq!(generate_moves(game.position(), &mut legal), 0);
        assert!(matches!(
            game.status_before_move_generation(),
            Some(GameStatus::Finished(GameResult::Draw(_)))
        ));

        let mut engine = ClassicEngine::default();
        let mut workspace = SearchWorkspace::new();
        let score = engine
            .negamax(
                &mut game,
                1,
                1,
                -SCORE_INF,
                SCORE_INF,
                false,
                0,
                Move::EMPTY,
                &mut workspace,
            )
            .expect("terminal immobility must not abort");
        assert_eq!(score, -SCORE_WIN + 1);
    }
}
