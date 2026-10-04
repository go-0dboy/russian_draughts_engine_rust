//! Classical deterministic search engine for Russian draughts.
//!
//! The module intentionally reuses the exact rule/move-generation core of the
//! AlphaZero project but has no neural-network dependency. The search is a
//! production-style iterative-deepening negamax/PVS alpha-beta engine with a
//! verified transposition table, aspiration windows, quiescence for mandatory
//! captures and promotions, move-ordering heuristics and a tunable handcrafted
//! evaluation.

mod corpus;
mod eval;
mod reference;
mod search;
mod search_params;
mod tt;

pub use corpus::{benchmark_suite, BenchmarkCase};
pub use eval::{
    eval_param_name, evaluation_phase, term_divisor, EvalBreakdown, EvalFeatures, EvalParams,
    Evaluator, PhaseScore, BASE_EVAL_TERM_COUNT, EVAL_FEATURE_COUNT, EVAL_PARAM_COUNT,
    EVAL_TERM_COUNT, KING_PSQT_OFFSET, MAN_PSQT_OFFSET, PHASE_SCALE,
};
pub use reference::{ReferenceSearchResult, ReferenceSearcher};
pub use search::{
    ClassicEngine, ClassicEngineBuildError, ClassicEngineConfig, ClassicSearchDecision,
    SearchControl, SearchLimits, SearchResult, SearchStats, SearchStopReason, MAX_SEARCH_PLY,
    SCORE_INF, SCORE_WIN, SCORE_WIN_THRESHOLD,
};
pub use search_params::{SearchParams, PRODUCTION_SEARCH_CONFIG, SEARCH_PARAM_NAMES};
pub use tt::{Bound as TtBound, ClassicKey, TranspositionTable, TtHit, TT_WAYS};
