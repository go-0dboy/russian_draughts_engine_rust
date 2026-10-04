//! Production line-oriented protocol adapter for `ClassicEngine`.
//!
//! Unlike the laboratory `examples/classic_server.rs`, this adapter keeps
//! reading commands while search runs so `stop`, `ping` and `quit` are usable
//! during a long analysis. The wire move format is stable `MoveSpec` data,
//! never the packed hot-path `Move(u64)` layout.

use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use russian_draughts_classic::api::{checked_position, CheckedGame, MoveSpec};
use russian_draughts_classic::{
    AutomaticDrawReason, ClassicEngine, ClassicEngineConfig, ClassicSearchControl as SearchControl,
    ClassicSearchDecision, ClassicSearchResult as SearchResult, ClassicSearchStopReason, DrawClaim,
    EndgameAdjudication, EvalParams, GameResult, GameStatus, SearchLimits, SearchParams, Side,
    ENGINE_NAME, ENGINE_VERSION,
};

const PROTOCOL_NAME: &str = "rd-classic";
const PROTOCOL_VERSION: u32 = 1;

enum Event {
    Command(String),
    InputClosed,
    SearchFinished(Box<ClassicEngine>, SearchResult),
}

fn main() {
    if let Err(err) = run() {
        eprintln!("classic_engine: {err}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let (eval, search, tt_megabytes) = parse_args()?;
    let mut game = CheckedGame::initial();
    let mut engine = Some(
        ClassicEngine::try_new(ClassicEngineConfig {
            tt_megabytes,
            eval,
            search,
            ..ClassicEngineConfig::default()
        })
        .map_err(|err| err.to_string())?,
    );
    let mut active_control = None;
    let mut quitting = false;

    let (tx, rx) = mpsc::channel::<Event>();
    spawn_input_thread(tx.clone());

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    writeln!(
        out,
        "ready {ENGINE_NAME} {ENGINE_VERSION} protocol {PROTOCOL_NAME}/{PROTOCOL_VERSION}"
    )
    .map_err(|err| err.to_string())?;
    out.flush().map_err(|err| err.to_string())?;

    event_loop(
        &rx,
        &tx,
        &mut out,
        &mut game,
        &mut engine,
        &mut active_control,
        &mut quitting,
    )
}

#[allow(clippy::too_many_arguments)]
fn event_loop(
    rx: &Receiver<Event>,
    tx: &Sender<Event>,
    out: &mut impl Write,
    game: &mut CheckedGame,
    engine: &mut Option<ClassicEngine>,
    active_control: &mut Option<SearchControl>,
    quitting: &mut bool,
) -> Result<(), String> {
    loop {
        let event = rx
            .recv()
            .map_err(|_| "protocol event channel closed".to_string())?;
        match event {
            Event::Command(line) => {
                if handle_command(&line, tx, out, game, engine, active_control, quitting)? {
                    return Ok(());
                }
            }
            Event::InputClosed => {
                if let Some(control) = active_control.as_ref() {
                    control.request_stop();
                    *quitting = true;
                } else {
                    return Ok(());
                }
            }
            Event::SearchFinished(returned_engine, result) => {
                *engine = Some(*returned_engine);
                *active_control = None;
                if !*quitting {
                    write_search_result(out, result)?;
                }
                if *quitting {
                    writeln!(out, "bye").map_err(|err| err.to_string())?;
                    out.flush().map_err(|err| err.to_string())?;
                    return Ok(());
                }
            }
        }
        out.flush().map_err(|err| err.to_string())?;
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_command(
    line: &str,
    tx: &Sender<Event>,
    out: &mut impl Write,
    game: &mut CheckedGame,
    engine: &mut Option<ClassicEngine>,
    active_control: &mut Option<SearchControl>,
    quitting: &mut bool,
) -> Result<bool, String> {
    let mut parts = line.split_whitespace();
    let Some(command) = parts.next() else {
        return Ok(false);
    };
    let busy = active_control.is_some();

    match command {
        "protocol" => {
            if let Err(message) = ensure_no_extra(&mut parts) {
                return write_error(out, &message);
            }
            writeln!(out, "protocol {PROTOCOL_NAME} {PROTOCOL_VERSION}")
                .map_err(|err| err.to_string())?;
        }
        "version" => {
            if let Err(message) = ensure_no_extra(&mut parts) {
                return write_error(out, &message);
            }
            writeln!(out, "version {ENGINE_VERSION}").map_err(|err| err.to_string())?;
        }
        "ping" => {
            let token = parts.next().unwrap_or("-");
            if let Err(message) = ensure_no_extra(&mut parts) {
                return write_error(out, &message);
            }
            writeln!(out, "pong {token}").map_err(|err| err.to_string())?;
        }
        "stop" => {
            if let Err(message) = ensure_no_extra(&mut parts) {
                return write_error(out, &message);
            }
            if let Some(control) = active_control.as_ref() {
                control.request_stop();
                writeln!(out, "ok stop").map_err(|err| err.to_string())?;
            } else {
                writeln!(out, "ok idle").map_err(|err| err.to_string())?;
            }
        }
        "quit" => {
            if let Err(message) = ensure_no_extra(&mut parts) {
                return write_error(out, &message);
            }
            if let Some(control) = active_control.as_ref() {
                control.request_stop();
                *quitting = true;
            } else {
                writeln!(out, "bye").map_err(|err| err.to_string())?;
                return Ok(true);
            }
        }
        _ if busy => {
            writeln!(out, "error busy").map_err(|err| err.to_string())?;
        }
        "new" => {
            if let Err(message) = ensure_no_extra(&mut parts) {
                return write_error(out, &message);
            }
            *game = CheckedGame::initial();
            engine
                .as_mut()
                .expect("idle protocol must own engine")
                .clear_transposition_table();
            writeln!(out, "ok").map_err(|err| err.to_string())?;
        }
        "position" => {
            let Some(white) = parse_hex_u32(parts.next()) else {
                return write_error(out, "position requires valid WHITE_HEX");
            };
            let Some(black) = parse_hex_u32(parts.next()) else {
                return write_error(out, "position requires valid BLACK_HEX");
            };
            let Some(kings) = parse_hex_u32(parts.next()) else {
                return write_error(out, "position requires valid KINGS_HEX");
            };
            let side = match parts.next() {
                Some("white") | Some("w") => Side::White,
                Some("black") | Some("b") => Side::Black,
                _ => return write_error(out, "position requires side white|black"),
            };
            if let Err(message) = ensure_no_extra(&mut parts) {
                return write_error(out, &message);
            }
            match checked_position(white, black, kings, side).and_then(CheckedGame::new) {
                Ok(new_game) => {
                    *game = new_game;
                    engine
                        .as_mut()
                        .expect("idle protocol must own engine")
                        .clear_transposition_table();
                    writeln!(out, "ok").map_err(|err| err.to_string())?;
                }
                Err(err) => return write_error(out, &format!("invalid_position {err}")),
            }
        }
        "legal" => {
            if let Err(message) = ensure_no_extra(&mut parts) {
                return write_error(out, &message);
            }
            let legal = game.legal_moves().map_err(|err| err.to_string())?;
            write!(out, "legal {}", legal.len()).map_err(|err| err.to_string())?;
            for spec in legal.iter() {
                write!(out, " ").map_err(|err| err.to_string())?;
                write_move_spec(out, spec)?;
            }
            writeln!(out).map_err(|err| err.to_string())?;
        }
        "play" => {
            let spec = match parse_move_spec(&mut parts) {
                Ok(spec) => spec,
                Err(message) => return write_error(out, &message),
            };
            match game.play(spec) {
                Ok(_) => writeln!(out, "ok").map_err(|err| err.to_string())?,
                Err(err) => return write_error(out, &format!("illegal_move {err}")),
            }
        }
        "side" => {
            if let Err(message) = ensure_no_extra(&mut parts) {
                return write_error(out, &message);
            }
            let side = match game.position().side_to_move {
                Side::White => "white",
                Side::Black => "black",
            };
            writeln!(out, "side {side}").map_err(|err| err.to_string())?;
        }
        "status" => {
            if let Err(message) = ensure_no_extra(&mut parts) {
                return write_error(out, &message);
            }
            write_status(out, game.game().status())?;
        }
        "go" | "analyze" => {
            let limits = match parse_limits(&mut parts) {
                Ok(limits) => limits,
                Err(message) => return write_error(out, &message),
            };
            let mut search_engine = engine.take().expect("idle protocol must own engine");
            let control = search_engine.control();
            *active_control = Some(control);
            let root = game.game().clone();
            let tx = tx.clone();
            thread::spawn(move || {
                let result = search_engine.search(&root, limits);
                let _ = tx.send(Event::SearchFinished(Box::new(search_engine), result));
            });
        }
        _ => {
            writeln!(out, "error unknown_command").map_err(|err| err.to_string())?;
        }
    }
    Ok(false)
}

fn spawn_input_thread(tx: Sender<Event>) {
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if tx.send(Event::Command(line)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(Event::InputClosed);
    });
}

fn parse_args() -> Result<(EvalParams, SearchParams, usize), String> {
    let mut eval = EvalParams::default();
    let mut search = SearchParams::default();
    let mut tt_megabytes = ClassicEngineConfig::default().tt_megabytes;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0usize;

    while i < args.len() {
        match args[i].as_str() {
            "--eval" => {
                let path = args.get(i + 1).ok_or("--eval requires a file path")?;
                let text = std::fs::read_to_string(path)
                    .map_err(|err| format!("cannot read eval file {path}: {err}"))?;
                eval = EvalParams::parse_kv(&text)
                    .map_err(|err| format!("invalid eval file {path}: {err}"))?;
                i += 2;
            }
            "--search" => {
                let path = args.get(i + 1).ok_or("--search requires a file path")?;
                let text = std::fs::read_to_string(path)
                    .map_err(|err| format!("cannot read search file {path}: {err}"))?;
                search = SearchParams::parse_kv(&text)
                    .map_err(|err| format!("invalid search file {path}: {err}"))?;
                i += 2;
            }
            "--tt" => {
                tt_megabytes = args
                    .get(i + 1)
                    .and_then(|value| value.parse::<usize>().ok())
                    .filter(|&value| value > 0)
                    .ok_or("--tt requires a positive size in MB")?;
                i += 2;
            }
            "-h" | "--help" => {
                println!(
                    "usage: classic_engine [--eval FILE] [--search FILE] [--tt MB]\n\
                     protocol commands are documented in PROTOCOL.md"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }

    Ok((eval, search, tt_megabytes))
}

fn parse_limits<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Result<SearchLimits, String> {
    let kind = parts
        .next()
        .ok_or_else(|| "go requires: nodes N [DEPTH] | time MS [DEPTH] | depth D".to_string())?;
    let limits = match kind {
        "nodes" => {
            let nodes = parse_positive_u64(parts.next(), "nodes requires positive N")?;
            let depth = parse_optional_positive_depth(parts.next())?;
            SearchLimits::nodes(depth, nodes)
        }
        "time" => {
            let ms = parse_positive_u64(parts.next(), "time requires positive MS")?;
            let depth = parse_optional_positive_depth(parts.next())?;
            SearchLimits::timed(depth, Duration::from_millis(ms))
        }
        "depth" => {
            let depth = parse_positive_depth(parts.next(), "depth requires positive D")?;
            SearchLimits::depth(depth)
        }
        _ => return Err("go requires: nodes N [DEPTH] | time MS [DEPTH] | depth D".into()),
    };
    ensure_no_extra(parts)?;
    Ok(limits)
}

fn parse_positive_u64(text: Option<&str>, message: &str) -> Result<u64, String> {
    let value = text
        .ok_or_else(|| message.to_string())?
        .parse::<u64>()
        .map_err(|_| message.to_string())?;
    (value > 0)
        .then_some(value)
        .ok_or_else(|| message.to_string())
}

fn parse_positive_depth(text: Option<&str>, message: &str) -> Result<u8, String> {
    let value = text
        .ok_or_else(|| message.to_string())?
        .parse::<u8>()
        .map_err(|_| message.to_string())?;
    (value > 0)
        .then_some(value)
        .ok_or_else(|| message.to_string())
}

fn parse_optional_positive_depth(text: Option<&str>) -> Result<u8, String> {
    match text {
        None => Ok(64),
        Some(value) => parse_positive_depth(Some(value), "optional DEPTH must be a positive u8"),
    }
}

fn parse_move_spec<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Result<MoveSpec, String> {
    let from = parse_protocol_square(parts.next(), "play requires valid FROM square 1..32")?;
    let to = parse_protocol_square(parts.next(), "play requires valid TO square 1..32")?;
    let captures_text = parts.next().ok_or_else(|| {
        "play requires CAPTURES ('-' or comma-list of unique squares 1..32)".to_string()
    })?;
    ensure_no_extra(parts)?;

    let mut captures = 0u32;
    if captures_text != "-" {
        if captures_text.is_empty() {
            return Err("capture list must not be empty".into());
        }
        for token in captures_text.split(',') {
            let sq = parse_protocol_square(Some(token), "capture squares must be 1..32")?;
            let bit = 1u32 << sq;
            if captures & bit != 0 {
                return Err("capture squares must be unique".into());
            }
            captures |= bit;
        }
    }
    Ok(MoveSpec { from, to, captures })
}

fn parse_protocol_square(text: Option<&str>, message: &str) -> Result<u8, String> {
    let one_based = text
        .ok_or_else(|| message.to_string())?
        .parse::<u8>()
        .map_err(|_| message.to_string())?;
    if !(1..=32).contains(&one_based) {
        return Err(message.into());
    }
    Ok(one_based - 1)
}

fn ensure_no_extra<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Result<(), String> {
    if parts.next().is_some() {
        Err("unexpected extra arguments".into())
    } else {
        Ok(())
    }
}

fn write_move_spec(out: &mut impl Write, spec: MoveSpec) -> Result<(), String> {
    write!(out, "{}:{}:", spec.from + 1, spec.to + 1).map_err(|err| err.to_string())?;
    if spec.captures == 0 {
        write!(out, "-").map_err(|err| err.to_string())?;
    } else {
        let mut first = true;
        let mut captures = spec.captures;
        while captures != 0 {
            let sq = captures.trailing_zeros() as u8;
            captures &= captures - 1;
            if !first {
                write!(out, ",").map_err(|err| err.to_string())?;
            }
            write!(out, "{}", sq + 1).map_err(|err| err.to_string())?;
            first = false;
        }
    }
    Ok(())
}

fn write_search_result(out: &mut impl Write, result: SearchResult) -> Result<(), String> {
    match result.decision {
        ClassicSearchDecision::Move(mv) => {
            write!(out, "bestmove ").map_err(|err| err.to_string())?;
            write_move_spec(out, MoveSpec::from_move(mv))?;
            writeln!(
                out,
                " score {} depth {} nodes {} time_us {} stop {}",
                result.score,
                result.depth,
                result.stats.nodes,
                result.elapsed.as_micros(),
                stop_reason_name(result.stop_reason)
            )
            .map_err(|err| err.to_string())?;
        }
        ClassicSearchDecision::ClaimDraw => {
            writeln!(
                out,
                "claim score {} depth {} nodes {}",
                result.score, result.depth, result.stats.nodes
            )
            .map_err(|err| err.to_string())?;
        }
        ClassicSearchDecision::ClaimDrawByMove(mv) => {
            write!(out, "claim_move ").map_err(|err| err.to_string())?;
            write_move_spec(out, MoveSpec::from_move(mv))?;
            writeln!(
                out,
                " score {} depth {} nodes {}",
                result.score, result.depth, result.stats.nodes
            )
            .map_err(|err| err.to_string())?;
        }
        ClassicSearchDecision::Terminal(result) => write_terminal(out, result)?,
        ClassicSearchDecision::NeedsAdjudication(reason) => {
            writeln!(out, "adjudication {}", adjudication_name(reason))
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

fn stop_reason_name(reason: ClassicSearchStopReason) -> &'static str {
    match reason {
        ClassicSearchStopReason::Completed => "completed",
        ClassicSearchStopReason::TimeLimit => "time_limit",
        ClassicSearchStopReason::NodeLimit => "node_limit",
        ClassicSearchStopReason::ExternalStop => "external_stop",
        ClassicSearchStopReason::HistoryLimit => "history_limit",
        ClassicSearchStopReason::ProvenWinOrLoss => "proven_win_or_loss",
    }
}

fn draw_reason_name(reason: AutomaticDrawReason) -> &'static str {
    match reason {
        AutomaticDrawReason::MaterialUnchangedLimit => "material_unchanged_limit",
        AutomaticDrawReason::KingOnlyLimit => "king_only_limit",
        AutomaticDrawReason::ThreeKingsVsLoneKingLimit => "three_kings_vs_lone_king_limit",
    }
}

fn draw_claim_name(claim: DrawClaim) -> &'static str {
    match claim {
        DrawClaim::ThreefoldRepetition => "threefold_repetition",
    }
}

fn adjudication_name(reason: EndgameAdjudication) -> &'static str {
    match reason {
        EndgameAdjudication::LoneKingOnBigRoadFiveMoveRule => "lone_king_big_road_five_move",
        EndgameAdjudication::ObviousDrawFiveMoveRule => "obvious_draw_five_move",
    }
}

fn write_status(out: &mut impl Write, status: GameStatus) -> Result<(), String> {
    match status {
        GameStatus::Ongoing => writeln!(out, "status ongoing"),
        GameStatus::ClaimableDraw(claim) => {
            writeln!(out, "status claimable {}", draw_claim_name(claim))
        }
        GameStatus::NeedsAdjudication(reason) => {
            writeln!(out, "status adjudication {}", adjudication_name(reason))
        }
        GameStatus::Finished(result) => return write_terminal(out, result),
    }
    .map_err(|err| err.to_string())
}

fn write_terminal(out: &mut impl Write, result: GameResult) -> Result<(), String> {
    match result {
        GameResult::Win(Side::White) => writeln!(out, "status white_win"),
        GameResult::Win(Side::Black) => writeln!(out, "status black_win"),
        GameResult::Draw(reason) => writeln!(out, "status draw {}", draw_reason_name(reason)),
    }
    .map_err(|err| err.to_string())
}

fn write_error(out: &mut impl Write, message: &str) -> Result<bool, String> {
    writeln!(out, "error {message}").map_err(|err| err.to_string())?;
    Ok(false)
}

fn parse_hex_u32(text: Option<&str>) -> Option<u32> {
    let text = text?;
    let trimmed = text.strip_prefix("0x").unwrap_or(text);
    u32::from_str_radix(trimmed, 16).ok()
}
