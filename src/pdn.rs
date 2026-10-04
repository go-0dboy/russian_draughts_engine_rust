//! Streaming PDN import for Russian draughts (GameType 25).
//!
//! The historical corpus used by the project is not guaranteed to be UTF-8,
//! so the reader works on raw bytes.  PDN syntax relevant to replay is ASCII;
//! human-readable header values are decoded lossily only when exposed.

use std::fmt;
use std::io::{self, BufRead};

use crate::{
    coordinates, generate_moves, square, Game, Move, Position, Side, DEFAULT_MOVE_BUFFER_CAPACITY,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdnOutcome {
    WhiteWin,
    Draw,
    BlackWin,
    Unknown,
}

impl PdnOutcome {
    pub const fn white_score(self) -> Option<f64> {
        match self {
            Self::WhiteWin => Some(1.0),
            Self::Draw => Some(0.5),
            Self::BlackWin => Some(0.0),
            Self::Unknown => None,
        }
    }

    pub const fn code(self) -> u8 {
        match self {
            Self::BlackWin => 0,
            Self::Draw => 1,
            Self::WhiteWin => 2,
            Self::Unknown => 3,
        }
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::BlackWin),
            1 => Some(Self::Draw),
            2 => Some(Self::WhiteWin),
            3 => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PdnMetadata {
    pub white: String,
    pub black: String,
    pub event: String,
    pub round: String,
    pub site: String,
    pub date: String,
    pub game_type: String,
    pub fen: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PdnGame {
    pub index: usize,
    pub metadata: PdnMetadata,
    pub outcome: PdnOutcome,
    pub start_position: Position,
    pub starts_from_fen: bool,
    pub moves: Vec<Move>,
}

impl PdnGame {
    pub fn replay(&self) -> Game {
        let mut game = Game::new(self.start_position);
        for &mv in &self.moves {
            game.play(mv);
        }
        game
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PdnErrorKind {
    InvalidHeader,
    UnsupportedGameType,
    InvalidResult,
    InvalidFen,
    InvalidMoveToken,
    IllegalMove,
    AmbiguousMove,
    ResultMismatch,
}

#[derive(Debug, Clone)]
pub struct PdnError {
    pub game_index: usize,
    pub ply: usize,
    pub kind: PdnErrorKind,
    pub message: String,
}

impl fmt::Display for PdnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PDN game {} ply {} {:?}: {}",
            self.game_index, self.ply, self.kind, self.message
        )
    }
}

impl std::error::Error for PdnError {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PdnStreamStats {
    pub games_seen: u64,
    pub games_ok: u64,
    pub games_rejected: u64,
}

#[derive(Default)]
struct RawHeaders {
    invalid_header: Option<String>,
    white: Vec<u8>,
    black: Vec<u8>,
    event: Vec<u8>,
    round: Vec<u8>,
    site: Vec<u8>,
    date: Vec<u8>,
    result: Vec<u8>,
    game_type: Vec<u8>,
    fen: Option<Vec<u8>>,
}

impl RawHeaders {
    fn set(&mut self, key: &[u8], value: Vec<u8>) {
        match key {
            b"White" => self.white = value,
            b"Black" => self.black = value,
            b"Event" => self.event = value,
            b"Round" => self.round = value,
            b"Site" => self.site = value,
            b"Date" => self.date = value,
            b"Result" => self.result = value,
            b"GameType" => self.game_type = value,
            b"FEN" => self.fen = Some(value),
            _ => {}
        }
    }
}

/// Stream a PDN file without requiring it to be valid UTF-8.  Each completed
/// game is replayed through the canonical move generator before being yielded.
/// A malformed game is reported to the callback and import continues at the
/// next header block.
pub fn for_each_pdn_game<R, F>(mut reader: R, mut callback: F) -> io::Result<PdnStreamStats>
where
    R: BufRead,
    F: FnMut(Result<PdnGame, PdnError>),
{
    let mut headers = RawHeaders::default();
    let mut movetext = Vec::<u8>::new();
    let mut line = Vec::<u8>::new();
    let mut have_headers = false;
    let mut in_movetext = false;
    let mut game_index = 0usize;
    let mut stats = PdnStreamStats::default();
    let mut lex_state = LineLexState::default();

    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line)?;
        if n == 0 {
            if have_headers || !movetext.is_empty() {
                game_index += 1;
                emit_game(
                    game_index,
                    &mut headers,
                    &mut movetext,
                    &mut stats,
                    &mut callback,
                );
            }
            break;
        }

        let trimmed = trim_ascii(&line);
        // A bracket-looking line inside a multi-line comment/variation is still
        // movetext. Only recognize a new tag at top lexical level.
        let is_header = lex_state.is_top_level() && looks_like_header_candidate(trimmed);

        if is_header && in_movetext {
            game_index += 1;
            emit_game(
                game_index,
                &mut headers,
                &mut movetext,
                &mut stats,
                &mut callback,
            );
            headers = RawHeaders::default();
            have_headers = false;
            in_movetext = false;
            lex_state = LineLexState::default();
        }

        if is_header {
            have_headers = true;
            match parse_header_line(trimmed) {
                Ok((key, value)) => headers.set(key, value.to_vec()),
                Err(message) => {
                    // Preserve the first syntax error until this game is emitted;
                    // later valid headers must not accidentally clear it.
                    if headers.invalid_header.is_none() {
                        headers.invalid_header = Some(message);
                    }
                }
            }
        } else if have_headers {
            if !trimmed.is_empty() {
                in_movetext = true;
            }
            movetext.extend_from_slice(&line);
            lex_state.scan(&line);
        }
    }

    Ok(stats)
}

fn emit_game<F: FnMut(Result<PdnGame, PdnError>)>(
    game_index: usize,
    headers: &mut RawHeaders,
    movetext: &mut Vec<u8>,
    stats: &mut PdnStreamStats,
    callback: &mut F,
) {
    stats.games_seen += 1;
    let result = parse_game(game_index, headers, movetext);
    if result.is_ok() {
        stats.games_ok += 1;
    } else {
        stats.games_rejected += 1;
    }
    callback(result);
    movetext.clear();
}

fn parse_game(index: usize, headers: &RawHeaders, movetext: &[u8]) -> Result<PdnGame, PdnError> {
    if let Some(message) = &headers.invalid_header {
        return Err(err(index, 0, PdnErrorKind::InvalidHeader, message.clone()));
    }

    if !headers.game_type.is_empty() && trim_ascii(&headers.game_type) != b"25" {
        return Err(err(
            index,
            0,
            PdnErrorKind::UnsupportedGameType,
            format!(
                "GameType={} is not Russian draughts (25)",
                String::from_utf8_lossy(&headers.game_type)
            ),
        ));
    }

    let header_outcome = parse_outcome(trim_ascii(&headers.result)).ok_or_else(|| {
        err(
            index,
            0,
            PdnErrorKind::InvalidResult,
            format!(
                "invalid Result tag {:?}",
                String::from_utf8_lossy(&headers.result)
            ),
        )
    })?;

    let (start_position, starts_from_fen) = if let Some(fen) = &headers.fen {
        (
            parse_fen(trim_ascii(fen))
                .map_err(|message| err(index, 0, PdnErrorKind::InvalidFen, message))?,
            true,
        )
    } else {
        (Position::initial(), false)
    };

    let tokens = mainline_tokens(movetext)
        .map_err(|message| err(index, 0, PdnErrorKind::InvalidMoveToken, message))?;
    let mut parsed_moves = Vec::<ParsedMove>::with_capacity(tokens.len() / 2 + 8);
    let mut movetext_outcome = None;

    for raw in tokens {
        let stripped = strip_move_number(&raw);
        if stripped.is_empty() || stripped[0] == b'$' {
            continue;
        }
        if let Some(outcome) = parse_outcome(stripped) {
            movetext_outcome = Some(outcome);
            continue;
        }

        let token = strip_annotations(stripped);
        if token.is_empty() {
            continue;
        }
        let parsed = parse_move_token(token).ok_or_else(|| {
            err(
                index,
                parsed_moves.len() + 1,
                PdnErrorKind::InvalidMoveToken,
                format!("cannot parse move token {}", String::from_utf8_lossy(token)),
            )
        })?;
        parsed_moves.push(parsed);
    }

    let mut game = Game::new(start_position);
    let moves = resolve_mainline(index, &mut game, &parsed_moves)?;

    if let Some(text_outcome) = movetext_outcome {
        if header_outcome != PdnOutcome::Unknown
            && text_outcome != PdnOutcome::Unknown
            && header_outcome != text_outcome
        {
            return Err(err(
                index,
                moves.len(),
                PdnErrorKind::ResultMismatch,
                format!(
                    "Result tag {:?} differs from movetext {:?}",
                    header_outcome, text_outcome
                ),
            ));
        }
    }
    let outcome = if header_outcome != PdnOutcome::Unknown {
        header_outcome
    } else {
        movetext_outcome.unwrap_or(PdnOutcome::Unknown)
    };

    Ok(PdnGame {
        index,
        metadata: PdnMetadata {
            white: lossy(&headers.white),
            black: lossy(&headers.black),
            event: lossy(&headers.event),
            round: lossy(&headers.round),
            site: lossy(&headers.site),
            date: lossy(&headers.date),
            game_type: lossy(&headers.game_type),
            fen: headers.fen.as_ref().map(|v| lossy(v)),
        },
        outcome,
        start_position,
        starts_from_fen,
        moves,
    })
}

/// Replay the parsed main line through the canonical move generator.  Most
/// PDN captures name only the first and last square.  If that notation maps to
/// several legal packed captures, use the following moves as look-ahead to
/// disambiguate.  Only an actually unique legal continuation is accepted.
fn resolve_mainline(
    game_index: usize,
    game: &mut Game,
    parsed: &[ParsedMove],
) -> Result<Vec<Move>, PdnError> {
    let mut moves = Vec::with_capacity(parsed.len());
    resolve_into(game_index, game, parsed, 0, &mut moves)?;
    Ok(moves)
}

fn resolve_into(
    game_index: usize,
    game: &mut Game,
    parsed: &[ParsedMove],
    ply: usize,
    output: &mut Vec<Move>,
) -> Result<(), PdnError> {
    if ply == parsed.len() {
        return Ok(());
    }

    let token = &parsed[ply];
    let route_captures = if token.capture && token.path.len() > 2 {
        Some(
            capture_mask_from_route(game.position(), &token.path).ok_or_else(|| {
                err(
                    game_index,
                    ply + 1,
                    PdnErrorKind::InvalidMoveToken,
                    "capture route is incompatible with the current position".to_string(),
                )
            })?,
        )
    } else {
        None
    };

    let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = generate_moves(game.position(), &mut legal);
    let mut candidates = [Move::EMPTY; 16];
    let mut candidate_len = 0usize;
    for &mv in &legal[..len] {
        if mv.from() == token.from()
            && mv.to() == token.to()
            && mv.is_capture() == token.capture
            && route_captures.map_or(true, |mask| mv.captures() == mask)
            && !candidates[..candidate_len].contains(&mv)
        {
            if candidate_len == candidates.len() {
                return Err(err(
                    game_index,
                    ply + 1,
                    PdnErrorKind::AmbiguousMove,
                    "too many equivalent PDN candidates".to_string(),
                ));
            }
            candidates[candidate_len] = mv;
            candidate_len += 1;
        }
    }

    if candidate_len == 0 {
        return Err(err(
            game_index,
            ply + 1,
            PdnErrorKind::IllegalMove,
            format!(
                "{}{}{} is not legal from {}",
                square_text(token.from()),
                if token.capture { 'x' } else { '-' },
                square_text(token.to()),
                position_text(game.position())
            ),
        ));
    }

    if candidate_len == 1 {
        let mv = candidates[0];
        game.play(mv);
        output.push(mv);
        return resolve_into(game_index, game, parsed, ply + 1, output);
    }

    let mut unique_solution: Option<(Game, Vec<Move>)> = None;
    for &mv in &candidates[..candidate_len] {
        let mut branch = game.clone();
        branch.play(mv);
        let mut branch_moves = Vec::with_capacity(parsed.len() - ply);
        branch_moves.push(mv);
        if resolve_into(game_index, &mut branch, parsed, ply + 1, &mut branch_moves).is_ok() {
            if unique_solution.is_some() {
                return Err(err(
                    game_index,
                    ply + 1,
                    PdnErrorKind::AmbiguousMove,
                    format!(
                        "{}{}{} has multiple legal continuations",
                        square_text(token.from()),
                        if token.capture { 'x' } else { '-' },
                        square_text(token.to())
                    ),
                ));
            }
            unique_solution = Some((branch, branch_moves));
        }
    }

    if let Some((resolved_game, branch_moves)) = unique_solution {
        *game = resolved_game;
        output.extend(branch_moves);
        Ok(())
    } else {
        Err(err(
            game_index,
            ply + 1,
            PdnErrorKind::AmbiguousMove,
            format!(
                "{}{}{} matches {} legal captures but none yields the recorded continuation",
                square_text(token.from()),
                if token.capture { 'x' } else { '-' },
                square_text(token.to()),
                candidate_len
            ),
        ))
    }
}

fn square_text(sq: u8) -> String {
    let (file, rank) = coordinates(sq).expect("valid square");
    format!("{}{}", (b'a' + file) as char, rank + 1)
}

#[derive(Debug, Clone)]
struct ParsedMove {
    path: Vec<u8>,
    capture: bool,
}

impl ParsedMove {
    #[inline]
    fn from(&self) -> u8 {
        self.path[0]
    }
    #[inline]
    fn to(&self) -> u8 {
        *self
            .path
            .last()
            .expect("move path has at least two squares")
    }
}

fn parse_move_token(token: &[u8]) -> Option<ParsedMove> {
    let mut path = Vec::with_capacity(6);
    let mut start = 0usize;
    let mut capture_kind = None::<bool>;

    for (i, &b) in token.iter().enumerate() {
        let this_capture = match b {
            b'x' | b'X' | b':' => true,
            b'-' => false,
            _ => continue,
        };
        if let Some(previous) = capture_kind {
            if previous != this_capture {
                return None;
            }
        } else {
            capture_kind = Some(this_capture);
        }
        path.push(parse_square_token(&token[start..i])?);
        start = i + 1;
    }

    let capture = capture_kind?;
    path.push(parse_square_token(&token[start..])?);
    if path.len() < 2 || (!capture && path.len() != 2) {
        return None;
    }
    Some(ParsedMove { path, capture })
}

/// Reconstruct the set of captured pieces from an explicitly written PDN
/// route (for example c3:e5:g7).  Russian draughts remove captured pieces only
/// after the whole move, so already captured pieces remain blockers here too.
fn capture_mask_from_route(pos: Position, path: &[u8]) -> Option<u32> {
    if path.len() < 2 {
        return None;
    }
    let side = pos.side_to_move;
    let mut ours = match side {
        Side::White => pos.white,
        Side::Black => pos.black,
    };
    let enemy = match side {
        Side::White => pos.black,
        Side::Black => pos.white,
    };
    let mut occupied = pos.occupied();
    let mut captured = 0u32;
    let mut current = path[0];
    let mut king = (pos.kings & (1u32 << current)) != 0;
    if (ours & (1u32 << current)) == 0 {
        return None;
    }

    for &landing in &path[1..] {
        let landing_bit = 1u32 << landing;
        if (occupied & landing_bit) != 0 {
            return None;
        }
        let (from_file, from_rank) = coordinates(current)?;
        let (to_file, to_rank) = coordinates(landing)?;
        let df = to_file as i8 - from_file as i8;
        let dr = to_rank as i8 - from_rank as i8;
        if df == 0 || df.unsigned_abs() != dr.unsigned_abs() {
            return None;
        }
        let distance = df.unsigned_abs();
        if !king && distance != 2 {
            return None;
        }
        let sf = if df > 0 { 1i8 } else { -1i8 };
        let sr = if dr > 0 { 1i8 } else { -1i8 };
        let mut victim = None::<u8>;
        for step in 1..distance {
            let file = (from_file as i8 + sf * step as i8) as u8;
            let rank = (from_rank as i8 + sr * step as i8) as u8;
            let sq = square(file, rank)?;
            let bit = 1u32 << sq;
            if (occupied & bit) == 0 {
                continue;
            }
            if (ours & bit) != 0 || (enemy & bit) == 0 || (captured & bit) != 0 || victim.is_some()
            {
                return None;
            }
            victim = Some(sq);
        }
        let victim = victim?;
        captured |= 1u32 << victim;

        let current_bit = 1u32 << current;
        ours = (ours & !current_bit) | landing_bit;
        occupied = (occupied & !current_bit) | landing_bit;
        current = landing;
        if !king {
            let promotion = match side {
                Side::White => (landing_bit & 0xF000_0000) != 0,
                Side::Black => (landing_bit & 0x0000_000F) != 0,
            };
            king = promotion;
        }
    }
    Some(captured)
}

fn parse_square_token(token: &[u8]) -> Option<u8> {
    let t = trim_ascii(token);
    if t.len() == 2 && (b'a'..=b'h').contains(&t[0].to_ascii_lowercase()) {
        let file = t[0].to_ascii_lowercase() - b'a';
        let rank_char = t[1];
        if !(b'1'..=b'8').contains(&rank_char) {
            return None;
        }
        return square(file, rank_char - b'1');
    }

    let mut value = 0u8;
    if t.is_empty() || t.len() > 2 {
        return None;
    }
    for &b in t {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(b - b'0')?;
    }
    pdn_number_to_square(value)
}

/// Standard 8x8 PDN numbering: 1=b8, 2=d8, ..., 29=a1, ..., 32=g1.
pub const fn pdn_number_to_square(number: u8) -> Option<u8> {
    if number == 0 || number > 32 {
        return None;
    }
    let idx = number - 1;
    let top_row = idx / 4;
    let pos = idx % 4;
    let rank = 7 - top_row;
    let file = pos * 2 + (rank & 1);
    square(file, rank)
}

pub fn parse_fen(fen: &[u8]) -> Result<Position, String> {
    let parts: Vec<&[u8]> = fen.split(|&b| b == b':').collect();
    if parts.len() != 3 {
        return Err("FEN must have side:W...:B...".to_string());
    }
    let side = match trim_ascii(parts[0]) {
        b"W" | b"w" => Side::White,
        b"B" | b"b" => Side::Black,
        _ => return Err("FEN side must be W or B".to_string()),
    };

    let mut white = 0u32;
    let mut black = 0u32;
    let mut kings = 0u32;
    for segment in &parts[1..] {
        let s = trim_ascii(segment);
        let Some((&color, rest)) = s.split_first() else {
            return Err("empty FEN color segment".to_string());
        };
        let is_white = match color {
            b'W' | b'w' => true,
            b'B' | b'b' => false,
            _ => return Err("FEN piece segment must start with W or B".to_string()),
        };
        if rest.is_empty() {
            continue;
        }
        for item in rest.split(|&b| b == b',') {
            let item = trim_ascii(item);
            if item.is_empty() {
                continue;
            }
            let (king, number_bytes) = if matches!(item.first().copied(), Some(b'K') | Some(b'k')) {
                (true, &item[1..])
            } else {
                (false, item)
            };
            let number = parse_u8_ascii(number_bytes)
                .ok_or_else(|| format!("invalid FEN square {}", String::from_utf8_lossy(item)))?;
            let sq = pdn_number_to_square(number)
                .ok_or_else(|| format!("FEN square {number} is outside 1..32"))?;
            let bit = 1u32 << sq;
            if ((white | black) & bit) != 0 {
                return Err(format!("duplicate/overlapping FEN square {number}"));
            }
            if is_white {
                white |= bit;
            } else {
                black |= bit;
            }
            if king {
                kings |= bit;
            }
        }
    }

    let pos = Position {
        white,
        black,
        kings,
        side_to_move: side,
    };
    if !pos.is_valid() {
        return Err("FEN describes an invalid board (overlap/kings/piece count)".to_string());
    }
    Ok(pos)
}

fn parse_outcome(text: &[u8]) -> Option<PdnOutcome> {
    match trim_ascii(text) {
        b"2-0" => Some(PdnOutcome::WhiteWin),
        b"1-1" => Some(PdnOutcome::Draw),
        b"0-2" => Some(PdnOutcome::BlackWin),
        b"*" | b"" => Some(PdnOutcome::Unknown),
        _ => None,
    }
}

fn mainline_tokens(input: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    let mut token = Vec::new();
    let mut variation_depth = 0u32;
    let mut brace_depth = 0u32;
    let mut semicolon_comment = false;

    let flush = |token: &mut Vec<u8>, out: &mut Vec<Vec<u8>>| {
        if !token.is_empty() {
            out.push(std::mem::take(token));
        }
    };

    for &b in input {
        if semicolon_comment {
            if b == b'\n' || b == b'\r' {
                semicolon_comment = false;
            }
            continue;
        }
        if brace_depth > 0 {
            match b {
                b'{' => brace_depth += 1,
                b'}' => brace_depth -= 1,
                _ => {}
            }
            continue;
        }
        if variation_depth > 0 {
            match b {
                b'{' => brace_depth = 1,
                b'(' => variation_depth += 1,
                b')' => variation_depth -= 1,
                b';' => semicolon_comment = true,
                _ => {}
            }
            continue;
        }

        match b {
            b'{' => {
                flush(&mut token, &mut out);
                brace_depth = 1;
            }
            b'(' => {
                flush(&mut token, &mut out);
                variation_depth = 1;
            }
            b';' => {
                flush(&mut token, &mut out);
                semicolon_comment = true;
            }
            b if b.is_ascii_whitespace() => flush(&mut token, &mut out),
            _ => token.push(b),
        }
    }
    flush(&mut token, &mut out);

    if brace_depth != 0 {
        return Err("unterminated {...} comment".to_string());
    }
    if variation_depth != 0 {
        return Err("unterminated (...) variation".to_string());
    }
    Ok(out)
}

fn strip_move_number(mut token: &[u8]) -> &[u8] {
    loop {
        let mut i = 0usize;
        while i < token.len() && token[i].is_ascii_digit() {
            i += 1;
        }
        if i == 0 || i >= token.len() || token[i] != b'.' {
            return token;
        }
        while i < token.len() && token[i] == b'.' {
            i += 1;
        }
        token = &token[i..];
        if token.is_empty() {
            return token;
        }
    }
}

fn strip_annotations(mut token: &[u8]) -> &[u8] {
    while let Some(&last) = token.last() {
        if matches!(last, b'!' | b'?' | b'+' | b'#') {
            token = &token[..token.len() - 1];
        } else {
            break;
        }
    }
    token
}

#[derive(Default)]
struct LineLexState {
    variation_depth: u32,
    brace_depth: u32,
    semicolon_comment: bool,
}

impl LineLexState {
    #[inline]
    fn is_top_level(&self) -> bool {
        self.variation_depth == 0 && self.brace_depth == 0 && !self.semicolon_comment
    }

    fn scan(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.semicolon_comment {
                if b == b'\n' || b == b'\r' {
                    self.semicolon_comment = false;
                }
                continue;
            }
            if self.brace_depth > 0 {
                match b {
                    b'{' => self.brace_depth += 1,
                    b'}' => self.brace_depth -= 1,
                    _ => {}
                }
                continue;
            }
            if self.variation_depth > 0 {
                match b {
                    b'{' => self.brace_depth = 1,
                    b'(' => self.variation_depth += 1,
                    b')' => self.variation_depth -= 1,
                    b';' => self.semicolon_comment = true,
                    _ => {}
                }
                continue;
            }
            match b {
                b'{' => self.brace_depth = 1,
                b'(' => self.variation_depth = 1,
                b';' => self.semicolon_comment = true,
                _ => {}
            }
        }
    }
}

fn looks_like_header_candidate(line: &[u8]) -> bool {
    let line = trim_ascii(line);
    line.len() >= 2 && line[0] == b'[' && line[1].is_ascii_alphabetic()
}

fn parse_header_line(line: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let line = trim_ascii(line);
    if line.len() < 4 || line[0] != b'[' || *line.last().unwrap() != b']' {
        return Err("malformed PDN header".to_string());
    }
    let body = trim_ascii(&line[1..line.len() - 1]);
    let key_end = body
        .iter()
        .position(|b| b.is_ascii_whitespace())
        .ok_or_else(|| "PDN header has no value".to_string())?;
    let key = &body[..key_end];
    let rest = trim_ascii(&body[key_end..]);
    if rest.len() < 2 || rest[0] != b'"' || *rest.last().unwrap() != b'"' {
        return Err("PDN header value must be quoted".to_string());
    }
    Ok((key, &rest[1..rest.len() - 1]))
}

fn position_text(pos: Position) -> String {
    format!(
        "W={:08x} B={:08x} K={:08x} stm={:?}",
        pos.white, pos.black, pos.kings, pos.side_to_move
    )
}

pub fn move_to_pdn(mv: Move) -> String {
    let (ff, fr) = coordinates(mv.from()).expect("valid from square");
    let (tf, tr) = coordinates(mv.to()).expect("valid to square");
    let sep = if mv.is_capture() { 'x' } else { '-' };
    format!(
        "{}{}{}{}{}",
        (b'a' + ff) as char,
        fr + 1,
        sep,
        (b'a' + tf) as char,
        tr + 1
    )
}

fn parse_u8_ascii(bytes: &[u8]) -> Option<u8> {
    let mut value = 0u8;
    if bytes.is_empty() {
        return None;
    }
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(b - b'0')?;
    }
    Some(value)
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(|b| b.is_ascii_whitespace()) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(|b| b.is_ascii_whitespace()) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(trim_ascii(bytes)).into_owned()
}

fn err(index: usize, ply: usize, kind: PdnErrorKind, message: String) -> PdnError {
    PdnError {
        game_index: index,
        ply,
        kind,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn pdn_numbering_matches_board_coordinates() {
        assert_eq!(pdn_number_to_square(1), square(1, 7)); // b8
        assert_eq!(pdn_number_to_square(12), square(7, 5)); // h6
        assert_eq!(pdn_number_to_square(29), square(0, 0)); // a1
        assert_eq!(pdn_number_to_square(32), square(6, 0)); // g1
    }

    #[test]
    fn parses_fen_with_kings() {
        let p = parse_fen(b"W:W5,K13,19,26,27,30:B1,12,20,21,K29").unwrap();
        assert_eq!(p.side_to_move, Side::White);
        assert_eq!(p.white.count_ones(), 6);
        assert_eq!(p.black.count_ones(), 5);
        assert_eq!(p.kings.count_ones(), 2);
    }

    #[test]
    fn ignores_comments_variations_and_nags() {
        let text = br#"[White "A"]
[Black "B"]
[Result "2-0"]
[GameType "25"]

1. c3-b4 {comment} b6-c5 2. b2-c3 (2. d2-c3 c7-b6) f6-e5 $1 2-0
"#;
        let mut games = Vec::new();
        let stats = for_each_pdn_game(Cursor::new(text), |g| games.push(g)).unwrap();
        assert_eq!(stats.games_seen, 1);
        let game = games.pop().unwrap().unwrap();
        assert_eq!(game.moves.len(), 4);
        assert_eq!(game.outcome, PdnOutcome::WhiteWin);
    }

    #[test]
    fn reads_non_utf8_header_values() {
        let mut bytes = b"[White \"".to_vec();
        bytes.extend_from_slice(&[0xcf, 0xe5, 0xf2, 0xf0, 0xee, 0xe2]);
        bytes.extend_from_slice(
            br#""]
[Black "B"]
[Result "1-1"]
[GameType "25"]

1. c3-b4 b6-c5 1-1
"#,
        );
        let mut ok = false;
        for_each_pdn_game(Cursor::new(bytes), |g| ok = g.is_ok()).unwrap();
        assert!(ok);
    }
    #[test]
    fn full_capture_route_reconstructs_capture_mask() {
        let c3 = square(2, 2).unwrap();
        let d4 = square(3, 3).unwrap();
        let e5 = square(4, 4).unwrap();
        let f6 = square(5, 5).unwrap();
        let g7 = square(6, 6).unwrap();
        let pos = Position {
            white: 1u32 << c3,
            black: (1u32 << d4) | (1u32 << f6),
            kings: 0,
            side_to_move: Side::White,
        };
        let mask = capture_mask_from_route(pos, &[c3, e5, g7]).unwrap();
        assert_eq!(mask, (1u32 << d4) | (1u32 << f6));
    }

    #[test]
    fn header_looking_line_inside_comment_is_not_a_header() {
        let text = br#"[White "A"]
[Black "B"]
[Result "1-1"]
[GameType "25"]

1. c3-b4 { historical note
[White "not a header"] still comment } b6-c5 1-1
"#;
        let mut games = Vec::new();
        let stats = for_each_pdn_game(Cursor::new(text), |g| games.push(g)).unwrap();
        assert_eq!(stats.games_seen, 1);
        assert_eq!(stats.games_rejected, 0);
        assert_eq!(games[0].as_ref().unwrap().moves.len(), 2);
    }

    #[test]
    fn later_header_cannot_clear_an_invalid_header() {
        let text = br#"[White A]
[Black "B"]
[Result "1-1"]
[GameType "25"]

1. c3-b4 b6-c5 1-1
"#;
        let mut games = Vec::new();
        let stats = for_each_pdn_game(Cursor::new(text), |g| games.push(g)).unwrap();
        assert_eq!(stats.games_rejected, 1);
        assert!(matches!(
            &games[0],
            Err(PdnError {
                kind: PdnErrorKind::InvalidHeader,
                ..
            })
        ));
    }

    #[test]
    fn replays_representative_historical_mainline() {
        let text = br#"[White "A"]
[Black "B"]
[Result "2-0"]
[GameType "25"]

1. c3-b4 b6-c5 2. b2-c3 f6-e5 3. a1-b2 g7-f6 4. e3-f4 f6-g5 5.
b4-a5 g5xe3 6. d2xf4 c7-b6 7. a5xc7 d8xb6 8. f4-g5 h6xf4 9.
a3-b4 c5xa3 10. c1-d2 a3xe3 11. f2xa5 2-0
"#;
        let mut games = Vec::new();
        let stats = for_each_pdn_game(Cursor::new(text), |g| games.push(g)).unwrap();
        assert_eq!(stats.games_ok, 1);
        let game = games.pop().unwrap().unwrap();
        assert_eq!(game.moves.len(), 21);
        assert_eq!(game.outcome, PdnOutcome::WhiteWin);
    }

    #[test]
    fn replays_black_to_move_fen_with_ellipsis_move_number() {
        let text = br#"[White "A"]
[Black "B"]
[Result "*"]
[GameType "25"]
[FEN "B:W18,19,21,22,23,25,26,27,28,32:B3,6,8,9,10,11,12,13,14,20"]

1... h6-g5 *
"#;
        let mut games = Vec::new();
        let stats = for_each_pdn_game(Cursor::new(text), |g| games.push(g)).unwrap();
        assert_eq!(stats.games_ok, 1);
        let game = games.pop().unwrap().unwrap();
        assert!(game.starts_from_fen);
        assert_eq!(game.start_position.side_to_move, Side::Black);
        assert_eq!(game.moves.len(), 1);
    }
}
