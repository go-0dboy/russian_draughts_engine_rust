use crate::tables::MAN_QUIET;
use crate::{count_legal_moves, king_attacks_from, Game, Move, Position, Side};

const CENTER_MASK: u32 = 0x0066_6600;
const INNER_CENTER_MASK: u32 = 0x0004_2000;
const WHITE_PRE_PROMOTION_RANK: u32 = 0x0F00_0000;
const WHITE_TWO_FROM_PROMOTION_RANK: u32 = 0x00F0_0000;
const BLACK_PRE_PROMOTION_RANK: u32 = 0x0000_00F0;
const BLACK_TWO_FROM_PROMOTION_RANK: u32 = 0x0000_0F00;
const EDGE_MASK: u32 = 0x8181_8181;
const LEFT_WING_MASK: u32 = 0x3333_3333;
const RIGHT_WING_MASK: u32 = 0xCCCC_CCCC;

const CONNECT_PLUS_3_SOURCES: u32 = 0x0E0E_0E0E;
const CONNECT_PLUS_4_SOURCES: u32 = 0x0FFF_FFFF;
const CONNECT_PLUS_5_SOURCES: u32 = 0x0070_7070;

pub const PHASE_SCALE: i32 = 256;
pub const LEGACY_TAPERED_V1_BASE_TERM_COUNT: usize = 20;
pub const BASE_EVAL_TERM_COUNT: usize = 28;
pub const MAN_PSQT_OFFSET: usize = BASE_EVAL_TERM_COUNT;
pub const KING_PSQT_OFFSET: usize = MAN_PSQT_OFFSET + 32;
pub const EVAL_TERM_COUNT: usize = KING_PSQT_OFFSET + 32;
pub const EVAL_PARAM_COUNT: usize = EVAL_TERM_COUNT * 2;

// Kept as a compatibility alias for tools that used the old name. In v1.3 it
// means the number of *raw terms*, while the number of independently tunable
// MG/EG weights is EVAL_PARAM_COUNT.
pub const EVAL_FEATURE_COUNT: usize = EVAL_TERM_COUNT;

const BASE_TERM_NAMES: [&str; BASE_EVAL_TERM_COUNT] = [
    "man_value",
    "king_value",
    "advancement_0",
    "advancement_1",
    "advancement_2",
    "advancement_3",
    "advancement_4",
    "advancement_5",
    "advancement_6",
    "advancement_7",
    "center_man",
    "center_king",
    "inner_center_bonus",
    "back_rank_guard",
    "mobility",
    "king_ray_mobility",
    "connected_piece",
    "blocked_man_penalty",
    "promotion_lane_bonus",
    "tempo",
    "isolated_man_penalty",
    "supported_man",
    "edge_isolated_man_penalty",
    "wing_imbalance_penalty",
    "advanced_supported_man",
    "man_quiet_mobility",
    "promotion_move",
    "constrained_king_penalty",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhaseScore {
    pub mg: i32,
    pub eg: i32,
}

impl PhaseScore {
    pub const fn new(mg: i32, eg: i32) -> Self {
        Self { mg, eg }
    }

    pub const fn same(value: i32) -> Self {
        Self {
            mg: value,
            eg: value,
        }
    }
}

/// Tapered handcrafted evaluation parameters.
///
/// Terms 0..20 are the v1.2 handcrafted features, 20..28 are the v1.4
/// structural/activity extension, 28..60 are a white-perspective man PSQT
/// and 60..92 a king PSQT. Black pieces use the
/// same tables after 180-degree rotation (`31 - square`). Every term has an
/// independent middlegame/endgame weight. The production default is the
/// expanded HCE accepted by both fixed-node and equal-time GSPRT in the
/// v1.4 line. `legacy_v12()` remains available for historical regression and
/// old-file parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalParams {
    terms: [PhaseScore; EVAL_TERM_COUNT],
}

impl EvalParams {
    pub const fn from_terms(terms: [PhaseScore; EVAL_TERM_COUNT]) -> Self {
        Self { terms }
    }

    pub const fn terms(&self) -> &[PhaseScore; EVAL_TERM_COUNT] {
        &self.terms
    }

    pub fn term_mut(&mut self, index: usize) -> &mut PhaseScore {
        &mut self.terms[index]
    }

    pub const fn term(&self, index: usize) -> PhaseScore {
        self.terms[index]
    }

    pub fn set_term_same(&mut self, index: usize, value: i32) {
        self.terms[index] = PhaseScore::same(value);
    }

    pub fn clear_non_material_terms(&mut self) {
        for term in &mut self.terms[2..] {
            *term = PhaseScore::same(0);
        }
    }

    pub fn to_vector(self) -> [i32; EVAL_PARAM_COUNT] {
        let mut out = [0i32; EVAL_PARAM_COUNT];
        let mut i = 0usize;
        while i < EVAL_TERM_COUNT {
            out[2 * i] = self.terms[i].mg;
            out[2 * i + 1] = self.terms[i].eg;
            i += 1;
        }
        out
    }

    pub fn from_vector(v: [i32; EVAL_PARAM_COUNT]) -> Self {
        let mut terms = [PhaseScore::same(0); EVAL_TERM_COUNT];
        let mut i = 0usize;
        while i < EVAL_TERM_COUNT {
            terms[i] = PhaseScore::new(v[2 * i], v[2 * i + 1]);
            i += 1;
        }
        Self { terms }
    }

    /// Parse both the v1.3 tapered format and the v1.2 scalar format. A scalar
    /// legacy key is copied to both phases, which makes old eval files valid
    /// baselines for same-binary A/B matches.
    pub fn parse_kv(text: &str) -> Result<Self, String> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum DeclaredFormat {
            Legacy,
            TaperedV1,
            TaperedV2,
        }

        let mut out = Self::legacy_v12();
        let mut declared_format = None::<DeclaredFormat>;
        let mut saw_legacy_key = false;
        let mut saw_tapered_key = false;
        let mut seen_param = [false; EVAL_PARAM_COUNT];

        fn mark_seen(
            seen: &mut [bool; EVAL_PARAM_COUNT],
            param: usize,
            line: usize,
            key: &str,
        ) -> Result<(), String> {
            if seen[param] {
                return Err(format!(
                    "line {line}: duplicate evaluation key/parameter {key}"
                ));
            }
            seen[param] = true;
            Ok(())
        }

        for (line_no, raw) in text.lines().enumerate() {
            let line_no = line_no + 1;
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("line {line_no}: expected key=value"));
            };
            let key = key.trim();
            let value = value.trim();
            if key == "format" {
                if declared_format.is_some() {
                    return Err(format!(
                        "line {line_no}: duplicate evaluation format declaration"
                    ));
                }
                declared_format = Some(match value {
                    "tapered_v1" => DeclaredFormat::TaperedV1,
                    "tapered_v2" => DeclaredFormat::TaperedV2,
                    "legacy_v1" => DeclaredFormat::Legacy,
                    _ => {
                        return Err(format!(
                            "line {line_no}: unsupported evaluation format {value}"
                        ))
                    }
                });
                continue;
            }

            if let Some((base, phase)) = key
                .strip_suffix("_mg")
                .map(|b| (b, 0usize))
                .or_else(|| key.strip_suffix("_eg").map(|b| (b, 1usize)))
            {
                saw_tapered_key = true;
                if base == "advancement" {
                    let values = parse_int_list(value, line_no, 8, base)?;
                    for (i, &parsed) in values.iter().enumerate() {
                        let term = 2 + i;
                        mark_seen(&mut seen_param, 2 * term + phase, line_no, key)?;
                        if phase == 0 {
                            out.terms[term].mg = parsed;
                        } else {
                            out.terms[term].eg = parsed;
                        }
                    }
                } else if base == "man_psqt" || base == "king_psqt" {
                    let values = parse_int_list(value, line_no, 32, base)?;
                    let offset = if base == "man_psqt" {
                        MAN_PSQT_OFFSET
                    } else {
                        KING_PSQT_OFFSET
                    };
                    for (i, &parsed) in values.iter().enumerate() {
                        let term = offset + i;
                        mark_seen(&mut seen_param, 2 * term + phase, line_no, key)?;
                        if phase == 0 {
                            out.terms[term].mg = parsed;
                        } else {
                            out.terms[term].eg = parsed;
                        }
                    }
                } else if let Some(index) = base_term_index(base) {
                    mark_seen(&mut seen_param, 2 * index + phase, line_no, key)?;
                    let parsed = parse_i32(value, line_no, base)?;
                    if phase == 0 {
                        out.terms[index].mg = parsed;
                    } else {
                        out.terms[index].eg = parsed;
                    }
                } else {
                    return Err(format!("line {line_no}: unknown evaluation key {key}"));
                }
                continue;
            }

            // Legacy scalar syntax. It deliberately updates both phases.
            saw_legacy_key = true;
            match key {
                "advancement" => {
                    let values = parse_int_list(value, line_no, 8, key)?;
                    for (i, &parsed) in values.iter().enumerate() {
                        let term = 2 + i;
                        mark_seen(&mut seen_param, 2 * term, line_no, key)?;
                        mark_seen(&mut seen_param, 2 * term + 1, line_no, key)?;
                        out.terms[term] = PhaseScore::same(parsed);
                    }
                }
                "man_psqt" | "king_psqt" => {
                    let values = parse_int_list(value, line_no, 32, key)?;
                    let offset = if key == "man_psqt" {
                        MAN_PSQT_OFFSET
                    } else {
                        KING_PSQT_OFFSET
                    };
                    for (i, &parsed) in values.iter().enumerate() {
                        let term = offset + i;
                        mark_seen(&mut seen_param, 2 * term, line_no, key)?;
                        mark_seen(&mut seen_param, 2 * term + 1, line_no, key)?;
                        out.terms[term] = PhaseScore::same(parsed);
                    }
                }
                _ => {
                    if let Some(index) = base_term_index(key) {
                        mark_seen(&mut seen_param, 2 * index, line_no, key)?;
                        mark_seen(&mut seen_param, 2 * index + 1, line_no, key)?;
                        out.terms[index] = PhaseScore::same(parse_i32(value, line_no, key)?);
                    } else {
                        return Err(format!("line {line_no}: unknown evaluation key {key}"));
                    }
                }
            }
        }

        match declared_format {
            Some(DeclaredFormat::TaperedV1) => {
                if saw_legacy_key {
                    return Err("tapered_v1 evaluation file contains legacy scalar keys".into());
                }
                if !saw_tapered_key {
                    return Err("tapered_v1 evaluation file contains no tapered parameters".into());
                }
                for (param, &seen) in seen_param.iter().enumerate() {
                    let term = param / 2;
                    let required =
                        !(LEGACY_TAPERED_V1_BASE_TERM_COUNT..MAN_PSQT_OFFSET).contains(&term);
                    if required && !seen {
                        return Err(format!(
                            "tapered_v1 evaluation file is incomplete: missing {}",
                            eval_param_name(param)
                        ));
                    }
                }
                for term in LEGACY_TAPERED_V1_BASE_TERM_COUNT..BASE_EVAL_TERM_COUNT {
                    if seen_param[2 * term] || seen_param[2 * term + 1] {
                        return Err(format!(
                            "tapered_v1 evaluation file contains v1.4-only parameter {}",
                            BASE_TERM_NAMES[term]
                        ));
                    }
                }
            }
            Some(DeclaredFormat::TaperedV2) => {
                if saw_legacy_key {
                    return Err("tapered_v2 evaluation file contains legacy scalar keys".into());
                }
                if !saw_tapered_key {
                    return Err("tapered_v2 evaluation file contains no tapered parameters".into());
                }
                if let Some(missing) = seen_param.iter().position(|&seen| !seen) {
                    return Err(format!(
                        "tapered_v2 evaluation file is incomplete: missing {}",
                        eval_param_name(missing)
                    ));
                }
            }
            Some(DeclaredFormat::Legacy) if saw_tapered_key => {
                return Err("legacy_v1 evaluation file contains tapered keys".into());
            }
            Some(DeclaredFormat::Legacy) => {}
            None => {
                // Old v1.2 files had no format marker. Keep them loadable. Mixed
                // unversioned files are accepted only when parameters do not
                // overlap; duplicate detection above keeps the result deterministic.
            }
        }

        Ok(out)
    }

    pub fn to_kv_string(self) -> String {
        let mut out = String::from("format=tapered_v2\n");
        for (index, &name) in BASE_TERM_NAMES.iter().enumerate() {
            if (2..10).contains(&index) {
                continue;
            }
            let t = self.terms[index];
            out.push_str(&format!("{name}_mg={}\n{name}_eg={}\n", t.mg, t.eg));
        }
        out.push_str("advancement_mg=");
        append_term_array(&mut out, &self.terms, 2, 8, true);
        out.push('\n');
        out.push_str("advancement_eg=");
        append_term_array(&mut out, &self.terms, 2, 8, false);
        out.push('\n');
        out.push_str("man_psqt_mg=");
        append_term_array(&mut out, &self.terms, MAN_PSQT_OFFSET, 32, true);
        out.push('\n');
        out.push_str("man_psqt_eg=");
        append_term_array(&mut out, &self.terms, MAN_PSQT_OFFSET, 32, false);
        out.push('\n');
        out.push_str("king_psqt_mg=");
        append_term_array(&mut out, &self.terms, KING_PSQT_OFFSET, 32, true);
        out.push('\n');
        out.push_str("king_psqt_eg=");
        append_term_array(&mut out, &self.terms, KING_PSQT_OFFSET, 32, false);
        out.push('\n');
        out
    }
}

impl EvalParams {
    /// Historical v1.2 HCE used for compatibility parsing and regression
    /// benchmarks. New engines should use `Default`, which is the accepted
    /// production HCE.
    pub const fn legacy_v12() -> Self {
        let mut terms = [PhaseScore::same(0); EVAL_TERM_COUNT];
        let legacy = [
            100, 290, 0, 2, 6, 12, 21, 33, 52, 0, 5, 3, 4, 5, 2, 1, 3, 4, 10, 3,
        ];
        let mut i = 0usize;
        while i < LEGACY_TAPERED_V1_BASE_TERM_COUNT {
            terms[i] = PhaseScore::same(legacy[i]);
            i += 1;
        }
        Self { terms }
    }

    /// HCE accepted by the v1.4 fixed-node and equal-time GSPRT gates.
    /// Values are intentionally embedded so the standalone engine uses the
    /// accepted baseline even when no external eval file is supplied.
    pub const fn production_v1() -> Self {
        let mut terms = [PhaseScore::same(0); EVAL_TERM_COUNT];
        terms[0] = PhaseScore::new(100, 100);
        terms[1] = PhaseScore::new(290, 290);
        terms[2] = PhaseScore::new(0, 0);
        terms[3] = PhaseScore::new(2, 2);
        terms[4] = PhaseScore::new(3, 2);
        terms[5] = PhaseScore::new(11, 11);
        terms[6] = PhaseScore::new(16, 16);
        terms[7] = PhaseScore::new(31, 31);
        terms[8] = PhaseScore::new(52, 52);
        terms[9] = PhaseScore::new(0, 0);
        terms[10] = PhaseScore::new(0, 3);
        terms[11] = PhaseScore::new(3, 3);
        terms[12] = PhaseScore::new(-1, 1);
        terms[13] = PhaseScore::new(11, 11);
        terms[14] = PhaseScore::new(-1, 7);
        terms[15] = PhaseScore::new(5, 7);
        terms[16] = PhaseScore::new(4, 4);
        terms[17] = PhaseScore::new(5, 6);
        terms[18] = PhaseScore::new(6, 8);
        terms[19] = PhaseScore::new(4, 4);
        terms[20] = PhaseScore::new(0, 0);
        terms[21] = PhaseScore::new(0, 0);
        terms[22] = PhaseScore::new(1, 1);
        terms[23] = PhaseScore::new(1, 2);
        terms[24] = PhaseScore::new(-1, 0);
        terms[25] = PhaseScore::new(1, 3);
        terms[26] = PhaseScore::new(0, 1);
        terms[27] = PhaseScore::new(0, 0);

        // Man PSQT. Every rank keeps zero mean, so PSQT does not absorb the
        // advancement term during offline tuning.
        terms[MAN_PSQT_OFFSET] = PhaseScore::new(-2, -2);
        terms[MAN_PSQT_OFFSET + 2] = PhaseScore::new(1, 0);
        terms[MAN_PSQT_OFFSET + 3] = PhaseScore::new(1, 2);
        terms[MAN_PSQT_OFFSET + 4] = PhaseScore::new(-1, 0);
        terms[MAN_PSQT_OFFSET + 5] = PhaseScore::new(1, 0);
        terms[MAN_PSQT_OFFSET + 12] = PhaseScore::new(-1, 0);
        terms[MAN_PSQT_OFFSET + 13] = PhaseScore::new(-1, 0);
        terms[MAN_PSQT_OFFSET + 15] = PhaseScore::new(2, 0);
        Self { terms }
    }
}

impl Default for EvalParams {
    fn default() -> Self {
        Self::production_v1()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalFeatures {
    pub values: [i16; EVAL_TERM_COUNT],
    /// 0 = pure endgame, 256 = pure middlegame/opening.
    pub phase: u16,
}

impl Default for EvalFeatures {
    fn default() -> Self {
        Self {
            values: [0; EVAL_TERM_COUNT],
            phase: 0,
        }
    }
}

impl EvalFeatures {
    #[inline]
    pub fn score_white(self, params: EvalParams) -> i32 {
        score_range(&self.values, self.phase, &params, 0, EVAL_TERM_COUNT)
    }

    #[inline]
    pub fn score_side_to_move(self, params: EvalParams, side: Side) -> i32 {
        let white = self.score_white(params);
        match side {
            Side::White => white,
            Side::Black => -white,
        }
    }
}

pub fn eval_param_name(index: usize) -> String {
    let term = index / 2;
    let phase = if index & 1 == 0 { "mg" } else { "eg" };
    if term < BASE_EVAL_TERM_COUNT {
        format!("{}_{}", BASE_TERM_NAMES[term], phase)
    } else if term < KING_PSQT_OFFSET {
        format!("man_psqt_{:02}_{}", term - MAN_PSQT_OFFSET, phase)
    } else if term < EVAL_TERM_COUNT {
        format!("king_psqt_{:02}_{}", term - KING_PSQT_OFFSET, phase)
    } else {
        format!("invalid_{index}")
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvalBreakdown {
    pub material: i32,
    pub advancement: i32,
    pub psqt: i32,
    pub center: i32,
    pub back_rank: i32,
    pub mobility: i32,
    pub king_mobility: i32,
    pub structure: i32,
    pub blocked_men: i32,
    pub promotion_pressure: i32,
    pub tempo: i32,
    pub isolated_men: i32,
    pub supported_men: i32,
    pub edge_isolated_men: i32,
    pub wing_balance: i32,
    pub advanced_supported_men: i32,
    pub man_mobility: i32,
    pub promotion_moves: i32,
    pub constrained_kings: i32,
    pub phase: u16,
    pub total_white: i32,
    pub side_to_move: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct Evaluator {
    params: EvalParams,
    psqt_active: bool,
    extended_active: bool,
}

impl Evaluator {
    pub fn new(params: EvalParams) -> Self {
        let psqt_active = params.terms[MAN_PSQT_OFFSET..]
            .iter()
            .any(|p| p.mg != 0 || p.eg != 0);
        let extended_active = params.terms[LEGACY_TAPERED_V1_BASE_TERM_COUNT..BASE_EVAL_TERM_COUNT]
            .iter()
            .any(|p| p.mg != 0 || p.eg != 0);
        Self {
            params,
            psqt_active,
            extended_active,
        }
    }

    pub fn evaluate(&self, game: &Game) -> EvalBreakdown {
        self.evaluate_with_current_moves(game, None)
    }

    /// Fast search-path score. This deliberately avoids building the 92-term
    /// feature vector and the diagnostic EvalBreakdown. It is required to be
    /// bit-exact with `evaluate(...).side_to_move` for the same parameters.
    #[inline]
    pub fn score(&self, game: &Game) -> i32 {
        self.score_with_current_moves(game, None)
    }

    #[inline]
    pub(crate) fn score_with_current_moves(
        &self,
        game: &Game,
        current_moves: Option<&[Move]>,
    ) -> i32 {
        let pos = game.position();
        let white_kings = pos.white & pos.kings;
        let black_kings = pos.black & pos.kings;
        let white_men = pos.white & !pos.kings;
        let black_men = pos.black & !pos.kings;
        let mut mg = 0i64;
        let mut eg = 0i64;

        #[inline(always)]
        fn add_term(mg: &mut i64, eg: &mut i64, feature: i32, p: PhaseScore) {
            if feature != 0 {
                *mg += feature as i64 * p.mg as i64;
                *eg += feature as i64 * p.eg as i64;
            }
        }

        add_term(
            &mut mg,
            &mut eg,
            white_men.count_ones() as i32 - black_men.count_ones() as i32,
            self.params.terms[0],
        );
        add_term(
            &mut mg,
            &mut eg,
            white_kings.count_ones() as i32 - black_kings.count_ones() as i32,
            self.params.terms[1],
        );

        // Men are walked once for both advancement and PSQT.
        let mut pieces = white_men;
        while pieces != 0 {
            let sq = pop_lsb(&mut pieces) as usize;
            let progress = sq / 4;
            add_term(&mut mg, &mut eg, 1, self.params.terms[2 + progress]);
            if self.psqt_active {
                add_term(&mut mg, &mut eg, 1, self.params.terms[MAN_PSQT_OFFSET + sq]);
            }
        }
        pieces = black_men;
        while pieces != 0 {
            let sq = pop_lsb(&mut pieces) as usize;
            let progress = 7 - sq / 4;
            add_term(&mut mg, &mut eg, -1, self.params.terms[2 + progress]);
            if self.psqt_active {
                add_term(
                    &mut mg,
                    &mut eg,
                    -1,
                    self.params.terms[MAN_PSQT_OFFSET + (31 - sq)],
                );
            }
        }

        if self.psqt_active {
            pieces = white_kings;
            while pieces != 0 {
                let sq = pop_lsb(&mut pieces) as usize;
                add_term(
                    &mut mg,
                    &mut eg,
                    1,
                    self.params.terms[KING_PSQT_OFFSET + sq],
                );
            }
            pieces = black_kings;
            while pieces != 0 {
                let sq = pop_lsb(&mut pieces) as usize;
                add_term(
                    &mut mg,
                    &mut eg,
                    -1,
                    self.params.terms[KING_PSQT_OFFSET + (31 - sq)],
                );
            }
        }

        let white_center = pos.white & CENTER_MASK;
        let black_center = pos.black & CENTER_MASK;
        add_term(
            &mut mg,
            &mut eg,
            (white_center & !white_kings).count_ones() as i32
                - (black_center & !black_kings).count_ones() as i32,
            self.params.terms[10],
        );
        add_term(
            &mut mg,
            &mut eg,
            (white_center & white_kings).count_ones() as i32
                - (black_center & black_kings).count_ones() as i32,
            self.params.terms[11],
        );
        add_term(
            &mut mg,
            &mut eg,
            (pos.white & INNER_CENTER_MASK).count_ones() as i32
                - (pos.black & INNER_CENTER_MASK).count_ones() as i32,
            self.params.terms[12],
        );

        // Preserve v1.2 semantics: the back-rank product is divided by 12 in
        // each phase before the final phase interpolation.
        let white_back = (white_men & 0x0000_000F).count_ones() as i32;
        let black_back = (black_men & 0xF000_0000).count_ones() as i32;
        let back_feature =
            white_back * black_men.count_ones() as i32 - black_back * white_men.count_ones() as i32;
        if back_feature != 0 {
            let p = self.params.terms[13];
            mg += back_feature as i64 * p.mg as i64 / 12;
            eg += back_feature as i64 * p.eg as i64 / 12;
        }

        let (white_legal, black_legal) = mobility_counts(pos, current_moves);
        add_term(
            &mut mg,
            &mut eg,
            white_legal.min(24) as i32 - black_legal.min(24) as i32,
            self.params.terms[14],
        );
        let (white_king_mobility, white_constrained_kings) = king_activity(pos, white_kings);
        let (black_king_mobility, black_constrained_kings) = king_activity(pos, black_kings);
        add_term(
            &mut mg,
            &mut eg,
            white_king_mobility - black_king_mobility,
            self.params.terms[15],
        );
        add_term(
            &mut mg,
            &mut eg,
            connected_bonds(pos.white) - connected_bonds(pos.black),
            self.params.terms[16],
        );
        add_term(
            &mut mg,
            &mut eg,
            blocked_men(pos, Side::Black) as i32 - blocked_men(pos, Side::White) as i32,
            self.params.terms[17],
        );
        add_term(
            &mut mg,
            &mut eg,
            promotion_pressure(white_men, Side::White) - promotion_pressure(black_men, Side::Black),
            self.params.terms[18],
        );
        add_term(
            &mut mg,
            &mut eg,
            match pos.side_to_move {
                Side::White => 1,
                Side::Black => -1,
            },
            self.params.terms[19],
        );

        if self.extended_active {
            let (white_isolated, white_supported, white_edge_isolated, white_advanced_supported) =
                man_structure(white_men, Side::White);
            let (black_isolated, black_supported, black_edge_isolated, black_advanced_supported) =
                man_structure(black_men, Side::Black);
            let (white_man_mobility, white_promotion_moves) =
                man_activity(pos, white_men, Side::White);
            let (black_man_mobility, black_promotion_moves) =
                man_activity(pos, black_men, Side::Black);
            add_term(
                &mut mg,
                &mut eg,
                black_isolated - white_isolated,
                self.params.terms[20],
            );
            add_term(
                &mut mg,
                &mut eg,
                white_supported - black_supported,
                self.params.terms[21],
            );
            add_term(
                &mut mg,
                &mut eg,
                black_edge_isolated - white_edge_isolated,
                self.params.terms[22],
            );
            add_term(
                &mut mg,
                &mut eg,
                wing_imbalance(black_men) - wing_imbalance(white_men),
                self.params.terms[23],
            );
            add_term(
                &mut mg,
                &mut eg,
                white_advanced_supported - black_advanced_supported,
                self.params.terms[24],
            );
            add_term(
                &mut mg,
                &mut eg,
                white_man_mobility - black_man_mobility,
                self.params.terms[25],
            );
            add_term(
                &mut mg,
                &mut eg,
                white_promotion_moves - black_promotion_moves,
                self.params.terms[26],
            );
            add_term(
                &mut mg,
                &mut eg,
                black_constrained_kings - white_constrained_kings,
                self.params.terms[27],
            );
        }

        let phase = evaluation_phase(pos) as i64;
        let white_score = div_round_signed(
            mg * phase + eg * (PHASE_SCALE as i64 - phase),
            PHASE_SCALE as i64,
        ) as i32;
        match pos.side_to_move {
            Side::White => white_score,
            Side::Black => -white_score,
        }
    }

    pub(crate) fn evaluate_with_current_moves(
        &self,
        game: &Game,
        current_moves: Option<&[Move]>,
    ) -> EvalBreakdown {
        let pos = game.position();
        let features = self.features_impl(game, current_moves, self.psqt_active);
        let total_white = features.score_white(self.params);
        EvalBreakdown {
            material: score_range(&features.values, features.phase, &self.params, 0, 2),
            advancement: score_range(&features.values, features.phase, &self.params, 2, 10),
            psqt: score_range(
                &features.values,
                features.phase,
                &self.params,
                MAN_PSQT_OFFSET,
                EVAL_TERM_COUNT,
            ),
            center: score_range(&features.values, features.phase, &self.params, 10, 13),
            back_rank: score_range(&features.values, features.phase, &self.params, 13, 14),
            mobility: score_range(&features.values, features.phase, &self.params, 14, 15),
            king_mobility: score_range(&features.values, features.phase, &self.params, 15, 16),
            structure: score_range(&features.values, features.phase, &self.params, 16, 17),
            blocked_men: score_range(&features.values, features.phase, &self.params, 17, 18),
            promotion_pressure: score_range(&features.values, features.phase, &self.params, 18, 19),
            tempo: score_range(&features.values, features.phase, &self.params, 19, 20),
            isolated_men: score_range(&features.values, features.phase, &self.params, 20, 21),
            supported_men: score_range(&features.values, features.phase, &self.params, 21, 22),
            edge_isolated_men: score_range(&features.values, features.phase, &self.params, 22, 23),
            wing_balance: score_range(&features.values, features.phase, &self.params, 23, 24),
            advanced_supported_men: score_range(
                &features.values,
                features.phase,
                &self.params,
                24,
                25,
            ),
            man_mobility: score_range(&features.values, features.phase, &self.params, 25, 26),
            promotion_moves: score_range(&features.values, features.phase, &self.params, 26, 27),
            constrained_kings: score_range(&features.values, features.phase, &self.params, 27, 28),
            phase: features.phase,
            // Use one final phase interpolation for the search score. Individual
            // breakdown components are diagnostic and can differ by a few units
            // because each component is rounded independently.
            total_white,
            side_to_move: match pos.side_to_move {
                Side::White => total_white,
                Side::Black => -total_white,
            },
        }
    }

    pub fn features(&self, game: &Game) -> EvalFeatures {
        self.features_impl(game, None, true)
    }

    /// Base handcrafted terms plus phase, without filling the 64 PSQT slots.
    /// Used by the offline tuner, which derives sparse PSQT gradients directly
    /// from the position bitboards and avoids unnecessary preprocessing work.
    pub fn base_features(&self, game: &Game) -> EvalFeatures {
        self.features_impl(game, None, false)
    }

    fn features_impl(
        &self,
        game: &Game,
        current_moves: Option<&[Move]>,
        include_psqt: bool,
    ) -> EvalFeatures {
        let pos = game.position();
        let white_kings = pos.white & pos.kings;
        let black_kings = pos.black & pos.kings;
        let white_men = pos.white & !pos.kings;
        let black_men = pos.black & !pos.kings;
        let mut v = [0i16; EVAL_TERM_COUNT];

        v[0] = (white_men.count_ones() as i32 - black_men.count_ones() as i32) as i16;
        v[1] = (white_kings.count_ones() as i32 - black_kings.count_ones() as i32) as i16;
        advancement_features(white_men, Side::White, 1, &mut v);
        advancement_features(black_men, Side::Black, -1, &mut v);

        let white_center = pos.white & CENTER_MASK;
        let black_center = pos.black & CENTER_MASK;
        v[10] = ((white_center & !white_kings).count_ones() as i32
            - (black_center & !black_kings).count_ones() as i32) as i16;
        v[11] = ((white_center & white_kings).count_ones() as i32
            - (black_center & black_kings).count_ones() as i32) as i16;
        v[12] = ((pos.white & INNER_CENTER_MASK).count_ones() as i32
            - (pos.black & INNER_CENTER_MASK).count_ones() as i32) as i16;

        let white_back = (white_men & 0x0000_000F).count_ones() as i32;
        let black_back = (black_men & 0xF000_0000).count_ones() as i32;
        let white_scale = black_men.count_ones() as i32;
        let black_scale = white_men.count_ones() as i32;
        v[13] = (white_back * white_scale - black_back * black_scale) as i16;

        let (white_legal, black_legal) = mobility_counts(pos, current_moves);
        v[14] = (white_legal.min(24) as i32 - black_legal.min(24) as i32) as i16;
        let (white_king_mobility, white_constrained_kings) = king_activity(pos, white_kings);
        let (black_king_mobility, black_constrained_kings) = king_activity(pos, black_kings);
        v[15] = (white_king_mobility - black_king_mobility) as i16;
        v[16] = (connected_bonds(pos.white) - connected_bonds(pos.black)) as i16;
        v[17] =
            (blocked_men(pos, Side::Black) as i32 - blocked_men(pos, Side::White) as i32) as i16;
        v[18] = (promotion_pressure(white_men, Side::White)
            - promotion_pressure(black_men, Side::Black)) as i16;
        v[19] = match pos.side_to_move {
            Side::White => 1,
            Side::Black => -1,
        };

        let (white_isolated, white_supported, white_edge_isolated, white_advanced_supported) =
            man_structure(white_men, Side::White);
        let (black_isolated, black_supported, black_edge_isolated, black_advanced_supported) =
            man_structure(black_men, Side::Black);
        let (white_man_mobility, white_promotion_moves) = man_activity(pos, white_men, Side::White);
        let (black_man_mobility, black_promotion_moves) = man_activity(pos, black_men, Side::Black);
        v[20] = (black_isolated - white_isolated) as i16;
        v[21] = (white_supported - black_supported) as i16;
        v[22] = (black_edge_isolated - white_edge_isolated) as i16;
        v[23] = (wing_imbalance(black_men) - wing_imbalance(white_men)) as i16;
        v[24] = (white_advanced_supported - black_advanced_supported) as i16;
        v[25] = (white_man_mobility - black_man_mobility) as i16;
        v[26] = (white_promotion_moves - black_promotion_moves) as i16;
        v[27] = (black_constrained_kings - white_constrained_kings) as i16;

        if include_psqt {
            let mut pieces = white_men;
            while pieces != 0 {
                let sq = pop_lsb(&mut pieces) as usize;
                v[MAN_PSQT_OFFSET + sq] += 1;
            }
            pieces = black_men;
            while pieces != 0 {
                let sq = pop_lsb(&mut pieces) as usize;
                v[MAN_PSQT_OFFSET + (31 - sq)] -= 1;
            }
            pieces = white_kings;
            while pieces != 0 {
                let sq = pop_lsb(&mut pieces) as usize;
                v[KING_PSQT_OFFSET + sq] += 1;
            }
            pieces = black_kings;
            while pieces != 0 {
                let sq = pop_lsb(&mut pieces) as usize;
                v[KING_PSQT_OFFSET + (31 - sq)] -= 1;
            }
        }

        EvalFeatures {
            values: v,
            phase: evaluation_phase(pos) as u16,
        }
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new(EvalParams::default())
    }
}

/// Phase model deliberately has no tunable constants yet. Men carry more
/// middlegame weight than kings, so promotion naturally moves the position
/// toward the endgame even when the piece count stays unchanged.
#[inline]
pub fn evaluation_phase(pos: Position) -> i32 {
    let pieces = pos.occupied().count_ones() as i32;
    let kings = (pos.kings & pos.occupied()).count_ones() as i32;
    let men = pieces - kings;
    let units = 2 * men + pieces; // initial position = 72, king-heavy positions much lower
    const MIN_UNITS: i32 = 6;
    const MAX_UNITS: i32 = 72;
    if units <= MIN_UNITS {
        0
    } else if units >= MAX_UNITS {
        PHASE_SCALE
    } else {
        (units - MIN_UNITS) * PHASE_SCALE / (MAX_UNITS - MIN_UNITS)
    }
}

#[inline]
fn score_range(
    values: &[i16; EVAL_TERM_COUNT],
    phase: u16,
    params: &EvalParams,
    start: usize,
    end: usize,
) -> i32 {
    let mg_phase = phase as i64;
    let eg_phase = (PHASE_SCALE - phase as i32) as i64;
    let mut mg_score = 0i64;
    let mut eg_score = 0i64;
    for (i, &value) in values.iter().enumerate().take(end).skip(start) {
        let f = i64::from(value);
        if f == 0 {
            continue;
        }
        let p = params.terms[i];
        let divisor = i64::from(term_divisor(i));
        // Preserve the v1.2 back-rank arithmetic exactly when MG == EG:
        // multiplication is truncated by /12 before phase interpolation.
        mg_score += f * p.mg as i64 / divisor;
        eg_score += f * p.eg as i64 / divisor;
    }
    let numerator = mg_score * mg_phase + eg_score * eg_phase;
    div_round_signed(numerator, PHASE_SCALE as i64) as i32
}

#[inline]
pub const fn term_divisor(index: usize) -> i32 {
    if index == 13 {
        12
    } else {
        1
    }
}

#[inline]
fn div_round_signed(value: i64, divisor: i64) -> i64 {
    if value >= 0 {
        (value + divisor / 2) / divisor
    } else {
        (value - divisor / 2) / divisor
    }
}

fn base_term_index(key: &str) -> Option<usize> {
    BASE_TERM_NAMES.iter().position(|&name| name == key)
}

fn parse_i32(value: &str, line: usize, name: &str) -> Result<i32, String> {
    value
        .parse::<i32>()
        .map_err(|_| format!("line {line}: invalid integer for {name}"))
}

fn parse_int_list(
    value: &str,
    line: usize,
    expected: usize,
    name: &str,
) -> Result<Vec<i32>, String> {
    let values = value
        .split(',')
        .map(str::trim)
        .map(|v| parse_i32(v, line, name))
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() != expected {
        return Err(format!(
            "line {line}: {name} requires exactly {expected} comma-separated integers"
        ));
    }
    Ok(values)
}

fn append_term_array(
    out: &mut String,
    terms: &[PhaseScore; EVAL_TERM_COUNT],
    offset: usize,
    count: usize,
    mg: bool,
) {
    for i in 0..count {
        if i != 0 {
            out.push(',');
        }
        let value = if mg {
            terms[offset + i].mg
        } else {
            terms[offset + i].eg
        };
        out.push_str(&value.to_string());
    }
}

fn advancement_features(mut men: u32, side: Side, sign: i16, out: &mut [i16; EVAL_TERM_COUNT]) {
    while men != 0 {
        let sq = pop_lsb(&mut men);
        let rank = sq / 4;
        let progress = match side {
            Side::White => rank,
            Side::Black => 7 - rank,
        } as usize;
        out[2 + progress] += sign;
    }
}

fn mobility_counts(pos: Position, current_moves: Option<&[Move]>) -> (usize, usize) {
    let white_count = if pos.side_to_move == Side::White {
        current_moves.map_or_else(
            || {
                let mut p = pos;
                p.side_to_move = Side::White;
                count_legal_moves(p)
            },
            |moves| moves.len(),
        )
    } else {
        let mut p = pos;
        p.side_to_move = Side::White;
        count_legal_moves(p)
    };
    let black_count = if pos.side_to_move == Side::Black {
        current_moves.map_or_else(
            || {
                let mut p = pos;
                p.side_to_move = Side::Black;
                count_legal_moves(p)
            },
            |moves| moves.len(),
        )
    } else {
        let mut p = pos;
        p.side_to_move = Side::Black;
        count_legal_moves(p)
    };
    (white_count, black_count)
}

fn king_activity(pos: Position, mut kings: u32) -> (i32, i32) {
    let occupied = pos.occupied();
    let mut mobility = 0i32;
    let mut constrained = 0i32;
    while kings != 0 {
        let sq = pop_lsb(&mut kings);
        let free = (king_attacks_from(sq, occupied) & !occupied).count_ones() as i32;
        mobility += free;
        constrained += (free <= 3) as i32;
    }
    (mobility, constrained)
}

#[inline]
fn man_structure(mut men: u32, side: Side) -> (i32, i32, i32, i32) {
    let all_men = men;
    let back_table = match side {
        Side::White => &MAN_QUIET[Side::Black as usize],
        Side::Black => &MAN_QUIET[Side::White as usize],
    };
    let mut isolated = 0i32;
    let mut supported = 0i32;
    let mut edge_isolated = 0i32;
    let mut advanced_supported = 0i32;
    while men != 0 {
        let sq = pop_lsb(&mut men) as usize;
        let bit = 1u32 << sq;
        let neighbours = MAN_QUIET[Side::White as usize][sq] | MAN_QUIET[Side::Black as usize][sq];
        let is_isolated = (neighbours & all_men) == 0;
        let is_supported = (back_table[sq] & all_men) != 0;
        isolated += is_isolated as i32;
        edge_isolated += (is_isolated && (bit & EDGE_MASK) != 0) as i32;
        supported += is_supported as i32;
        let rank = sq / 4;
        let progress = match side {
            Side::White => rank,
            Side::Black => 7 - rank,
        };
        advanced_supported += (is_supported && progress >= 4) as i32;
    }
    (isolated, supported, edge_isolated, advanced_supported)
}

#[inline(always)]
fn wing_imbalance(men: u32) -> i32 {
    let left = (men & LEFT_WING_MASK).count_ones() as i32;
    let right = (men & RIGHT_WING_MASK).count_ones() as i32;
    (left - right).abs()
}

#[inline]
fn man_activity(pos: Position, mut men: u32, side: Side) -> (i32, i32) {
    let empty = !pos.occupied();
    let quiet_table = &MAN_QUIET[side as usize];
    let pre_promotion = match side {
        Side::White => WHITE_PRE_PROMOTION_RANK,
        Side::Black => BLACK_PRE_PROMOTION_RANK,
    };
    let mut quiet = 0i32;
    let mut promotion = 0i32;
    while men != 0 {
        let sq = pop_lsb(&mut men) as usize;
        let destinations = quiet_table[sq] & empty;
        let count = destinations.count_ones() as i32;
        quiet += count;
        if (pre_promotion & (1u32 << sq)) != 0 {
            promotion += count;
        }
    }
    (quiet, promotion)
}

#[inline(always)]
fn connected_bonds(pieces: u32) -> i32 {
    let plus_3 = ((pieces & CONNECT_PLUS_3_SOURCES) & (pieces >> 3)).count_ones();
    let plus_4 = ((pieces & CONNECT_PLUS_4_SOURCES) & (pieces >> 4)).count_ones();
    let plus_5 = ((pieces & CONNECT_PLUS_5_SOURCES) & (pieces >> 5)).count_ones();
    (plus_3 + plus_4 + plus_5) as i32
}

#[inline]
fn blocked_men(pos: Position, side: Side) -> u32 {
    let ours = match side {
        Side::White => pos.white,
        Side::Black => pos.black,
    };
    let mut men = ours & !pos.kings;
    let empty = !pos.occupied();
    let mut blocked = 0u32;
    let table = &MAN_QUIET[side as usize];
    while men != 0 {
        let sq = pop_lsb(&mut men);
        blocked += ((table[sq as usize] & empty) == 0) as u32;
    }
    blocked
}

#[inline(always)]
fn promotion_pressure(men: u32, side: Side) -> i32 {
    let (pre, two_away) = match side {
        Side::White => (WHITE_PRE_PROMOTION_RANK, WHITE_TWO_FROM_PROMOTION_RANK),
        Side::Black => (BLACK_PRE_PROMOTION_RANK, BLACK_TWO_FROM_PROMOTION_RANK),
    };
    2 * (men & pre).count_ones() as i32 + (men & two_away).count_ones() as i32
}

#[inline(always)]
fn pop_lsb(bb: &mut u32) -> u8 {
    let sq = bb.trailing_zeros() as u8;
    *bb &= *bb - 1;
    sq
}

#[cfg(test)]
mod fast_path_tests {
    use super::*;
    use crate::{generate_moves, DEFAULT_MOVE_BUFFER_CAPACITY};

    #[test]
    fn fast_score_with_reused_current_moves_matches_diagnostic_path() {
        let mut params = EvalParams::default();
        *params.term_mut(14) = PhaseScore::new(1, 5);
        *params.term_mut(MAN_PSQT_OFFSET + 5) = PhaseScore::new(2, -1);
        *params.term_mut(KING_PSQT_OFFSET + 11) = PhaseScore::new(-2, 4);
        let evaluator = Evaluator::new(params);
        let game = Game::new(Position::initial());
        let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let len = generate_moves(game.position(), &mut moves);
        assert_eq!(
            evaluator.score_with_current_moves(&game, Some(&moves[..len])),
            evaluator
                .evaluate_with_current_moves(&game, Some(&moves[..len]))
                .side_to_move,
        );
    }
}
