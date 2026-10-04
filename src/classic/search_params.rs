//! Versioned search parameters used by the production engine and SPSA lab.
//!
//! The production default is sourced from `config/search_default.txt`. Search
//! algorithms stay in `search.rs`; this module owns numeric policy knobs and
//! their strict text representation. Accepting a tuned baseline therefore
//! changes one configuration file rather than duplicating values in Rust code.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchParams {
    pub aspiration_window: i32,
    pub aspiration_min_depth: u8,
    pub max_extensions_per_line: u8,
    pub qsearch_promotion_depth: u8,
    pub forced_extension_min_depth: i16,
    pub promotion_extension_max_depth: i16,
    pub lmr_min_depth: i16,
    /// One-based move number. `4` means the fourth and later legal move.
    pub lmr_min_move: u8,
    pub lmr_second_depth: i16,
    /// One-based move number for the second reduction step.
    pub lmr_second_move: u8,
    pub lmr_third_depth: i16,
    /// One-based move number for the third reduction step.
    pub lmr_third_move: u8,
    pub lmr_base_reduction: i16,
    pub lmr_second_reduction: i16,
    pub lmr_third_reduction: i16,
    pub history_bonus_scale: i32,
    pub history_bonus_max: i32,
    pub history_malus_divisor: i32,
    pub enable_pvs: bool,
    pub enable_lmr: bool,
}

pub const PRODUCTION_SEARCH_CONFIG: &str = include_str!("../../config/search_default.txt");

impl Default for SearchParams {
    fn default() -> Self {
        Self::parse_kv(PRODUCTION_SEARCH_CONFIG)
            .expect("embedded config/search_default.txt must be a valid complete search_v1 file")
    }
}

impl SearchParams {
    /// Neutral parser seed only. Every `search_v1` field is mandatory, so none
    /// of these values can leak into a successfully parsed configuration.
    const fn parser_seed() -> Self {
        Self {
            aspiration_window: 0,
            aspiration_min_depth: 0,
            max_extensions_per_line: 0,
            qsearch_promotion_depth: 0,
            forced_extension_min_depth: 0,
            promotion_extension_max_depth: 0,
            lmr_min_depth: 0,
            lmr_min_move: 0,
            lmr_second_depth: 0,
            lmr_second_move: 0,
            lmr_third_depth: 0,
            lmr_third_move: 0,
            lmr_base_reduction: 0,
            lmr_second_reduction: 0,
            lmr_third_reduction: 0,
            history_bonus_scale: 0,
            history_bonus_max: 0,
            history_malus_divisor: 0,
            enable_pvs: false,
            enable_lmr: false,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if !(8..=32_000).contains(&self.aspiration_window) {
            return Err("aspiration_window must be in 8..=32000".into());
        }
        if !(1..=32).contains(&self.aspiration_min_depth) {
            return Err("aspiration_min_depth must be in 1..=32".into());
        }
        if self.max_extensions_per_line > 8 {
            return Err("max_extensions_per_line must be in 0..=8".into());
        }
        if self.qsearch_promotion_depth > 8 {
            return Err("qsearch_promotion_depth must be in 0..=8".into());
        }
        if !(1..=32).contains(&self.forced_extension_min_depth) {
            return Err("forced_extension_min_depth must be in 1..=32".into());
        }
        if !(0..=32).contains(&self.promotion_extension_max_depth) {
            return Err("promotion_extension_max_depth must be in 0..=32".into());
        }
        if !(2..=32).contains(&self.lmr_min_depth) {
            return Err("lmr_min_depth must be in 2..=32".into());
        }
        if !(2..=64).contains(&self.lmr_min_move) {
            return Err("lmr_min_move must be in 2..=64".into());
        }
        if !(self.lmr_min_depth..=48).contains(&self.lmr_second_depth) {
            return Err("lmr_second_depth must be >= lmr_min_depth and <= 48".into());
        }
        if !(self.lmr_min_move..=64).contains(&self.lmr_second_move) {
            return Err("lmr_second_move must be >= lmr_min_move and <= 64".into());
        }
        if !(self.lmr_second_depth..=64).contains(&self.lmr_third_depth) {
            return Err("lmr_third_depth must be >= lmr_second_depth and <= 64".into());
        }
        if !(self.lmr_second_move..=64).contains(&self.lmr_third_move) {
            return Err("lmr_third_move must be >= lmr_second_move and <= 64".into());
        }
        for (name, value) in [
            ("lmr_base_reduction", self.lmr_base_reduction),
            ("lmr_second_reduction", self.lmr_second_reduction),
            ("lmr_third_reduction", self.lmr_third_reduction),
        ] {
            if !(0..=4).contains(&value) {
                return Err(format!("{name} must be in 0..=4"));
            }
        }
        if !(1..=512).contains(&self.history_bonus_scale) {
            return Err("history_bonus_scale must be in 1..=512".into());
        }
        if !(64..=16_384).contains(&self.history_bonus_max) {
            return Err("history_bonus_max must be in 64..=16384".into());
        }
        if !(1..=16).contains(&self.history_malus_divisor) {
            return Err("history_malus_divisor must be in 1..=16".into());
        }
        Ok(())
    }

    pub fn parse_kv(text: &str) -> Result<Self, String> {
        const N: usize = 20;
        let mut out = Self::parser_seed();
        let mut seen = [false; N];
        let mut saw_format = false;

        fn set_seen(seen: &mut [bool], index: usize, line: usize, key: &str) -> Result<(), String> {
            if seen[index] {
                return Err(format!("line {line}: duplicate search parameter {key}"));
            }
            seen[index] = true;
            Ok(())
        }
        fn int<T: std::str::FromStr>(value: &str, line: usize, key: &str) -> Result<T, String> {
            value
                .parse::<T>()
                .map_err(|_| format!("line {line}: invalid integer for {key}: {value}"))
        }
        fn boolean(value: &str, line: usize, key: &str) -> Result<bool, String> {
            match value {
                "true" | "1" => Ok(true),
                "false" | "0" => Ok(false),
                _ => Err(format!("line {line}: invalid boolean for {key}: {value}")),
            }
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
                if saw_format {
                    return Err(format!(
                        "line {line_no}: duplicate search format declaration"
                    ));
                }
                if value != "search_v1" {
                    return Err(format!("line {line_no}: unsupported search format {value}"));
                }
                saw_format = true;
                continue;
            }

            macro_rules! assign {
                ($idx:expr, $field:ident, $ty:ty) => {{
                    set_seen(&mut seen, $idx, line_no, key)?;
                    out.$field = int::<$ty>(value, line_no, key)?;
                }};
            }
            match key {
                "aspiration_window" => assign!(0, aspiration_window, i32),
                "aspiration_min_depth" => assign!(1, aspiration_min_depth, u8),
                "max_extensions_per_line" => assign!(2, max_extensions_per_line, u8),
                "qsearch_promotion_depth" => assign!(3, qsearch_promotion_depth, u8),
                "forced_extension_min_depth" => assign!(4, forced_extension_min_depth, i16),
                "promotion_extension_max_depth" => assign!(5, promotion_extension_max_depth, i16),
                "lmr_min_depth" => assign!(6, lmr_min_depth, i16),
                "lmr_min_move" => assign!(7, lmr_min_move, u8),
                "lmr_second_depth" => assign!(8, lmr_second_depth, i16),
                "lmr_second_move" => assign!(9, lmr_second_move, u8),
                "lmr_third_depth" => assign!(10, lmr_third_depth, i16),
                "lmr_third_move" => assign!(11, lmr_third_move, u8),
                "lmr_base_reduction" => assign!(12, lmr_base_reduction, i16),
                "lmr_second_reduction" => assign!(13, lmr_second_reduction, i16),
                "lmr_third_reduction" => assign!(14, lmr_third_reduction, i16),
                "history_bonus_scale" => assign!(15, history_bonus_scale, i32),
                "history_bonus_max" => assign!(16, history_bonus_max, i32),
                "history_malus_divisor" => assign!(17, history_malus_divisor, i32),
                "enable_pvs" => {
                    set_seen(&mut seen, 18, line_no, key)?;
                    out.enable_pvs = boolean(value, line_no, key)?;
                }
                "enable_lmr" => {
                    set_seen(&mut seen, 19, line_no, key)?;
                    out.enable_lmr = boolean(value, line_no, key)?;
                }
                _ => return Err(format!("line {line_no}: unknown search parameter {key}")),
            }
        }

        if !saw_format {
            return Err("search parameter file is missing format=search_v1".into());
        }
        if let Some(index) = seen.iter().position(|&x| !x) {
            return Err(format!(
                "search_v1 file is incomplete: missing {}",
                SEARCH_PARAM_NAMES[index]
            ));
        }
        out.validate()?;
        Ok(out)
    }

    pub fn to_kv_string(self) -> String {
        format!(
            concat!(
                "format=search_v1\n",
                "aspiration_window={}\n",
                "aspiration_min_depth={}\n",
                "max_extensions_per_line={}\n",
                "qsearch_promotion_depth={}\n",
                "forced_extension_min_depth={}\n",
                "promotion_extension_max_depth={}\n",
                "lmr_min_depth={}\n",
                "lmr_min_move={}\n",
                "lmr_second_depth={}\n",
                "lmr_second_move={}\n",
                "lmr_third_depth={}\n",
                "lmr_third_move={}\n",
                "lmr_base_reduction={}\n",
                "lmr_second_reduction={}\n",
                "lmr_third_reduction={}\n",
                "history_bonus_scale={}\n",
                "history_bonus_max={}\n",
                "history_malus_divisor={}\n",
                "enable_pvs={}\n",
                "enable_lmr={}\n"
            ),
            self.aspiration_window,
            self.aspiration_min_depth,
            self.max_extensions_per_line,
            self.qsearch_promotion_depth,
            self.forced_extension_min_depth,
            self.promotion_extension_max_depth,
            self.lmr_min_depth,
            self.lmr_min_move,
            self.lmr_second_depth,
            self.lmr_second_move,
            self.lmr_third_depth,
            self.lmr_third_move,
            self.lmr_base_reduction,
            self.lmr_second_reduction,
            self.lmr_third_reduction,
            self.history_bonus_scale,
            self.history_bonus_max,
            self.history_malus_divisor,
            self.enable_pvs,
            self.enable_lmr,
        )
    }
}

pub const SEARCH_PARAM_NAMES: [&str; 20] = [
    "aspiration_window",
    "aspiration_min_depth",
    "max_extensions_per_line",
    "qsearch_promotion_depth",
    "forced_extension_min_depth",
    "promotion_extension_max_depth",
    "lmr_min_depth",
    "lmr_min_move",
    "lmr_second_depth",
    "lmr_second_move",
    "lmr_third_depth",
    "lmr_third_move",
    "lmr_base_reduction",
    "lmr_second_reduction",
    "lmr_third_reduction",
    "history_bonus_scale",
    "history_bonus_max",
    "history_malus_divisor",
    "enable_pvs",
    "enable_lmr",
];
