use std::io::{self, BufRead, Write};
use std::time::Duration;

use russian_draughts_classic::{
    generate_moves, ClassicEngine, ClassicEngineConfig, ClassicSearchDecision, EvalParams, Game,
    GameResult, GameStatus, Move, SearchLimits, SearchParams, Side, DEFAULT_MOVE_BUFFER_CAPACITY,
    ENGINE_VERSION,
};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut eval = EvalParams::default();
    let mut tt_megabytes = ClassicEngineConfig::default().tt_megabytes;
    let mut search = SearchParams::default();
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|arg| arg == "--eval") {
        let path = args
            .get(pos + 1)
            .unwrap_or_else(|| fail("--eval requires a file path"));
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|err| fail(&format!("cannot read eval file {path}: {err}")));
        eval = EvalParams::parse_kv(&text)
            .unwrap_or_else(|err| fail(&format!("invalid eval file {path}: {err}")));
    }
    if let Some(pos) = args.iter().position(|arg| arg == "--search") {
        let path = args
            .get(pos + 1)
            .unwrap_or_else(|| fail("--search requires a file path"));
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|err| fail(&format!("cannot read search file {path}: {err}")));
        search = SearchParams::parse_kv(&text)
            .unwrap_or_else(|err| fail(&format!("invalid search file {path}: {err}")));
    }
    if let Some(pos) = args.iter().position(|arg| arg == "--tt") {
        tt_megabytes = args
            .get(pos + 1)
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or_else(|| fail("--tt requires a positive size in MB"));
    }

    let mut game = Game::initial();
    let mut engine = ClassicEngine::new(ClassicEngineConfig {
        tt_megabytes,
        eval,
        search,
        ..ClassicEngineConfig::default()
    });

    writeln!(stdout, "ready {ENGINE_VERSION}").expect("stdout");
    stdout.flush().expect("flush");

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        let mut parts = line.split_whitespace();
        let Some(command) = parts.next() else {
            continue;
        };

        match command {
            "new" => {
                game = Game::initial();
                engine.clear_transposition_table();
                writeln!(stdout, "ok").expect("stdout");
            }
            "legal" => {
                let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
                let len = generate_moves(game.position(), &mut moves);
                write!(stdout, "legal {len}").expect("stdout");
                for mv in &moves[..len] {
                    write!(stdout, " {:016x}", mv.raw()).expect("stdout");
                }
                writeln!(stdout).expect("stdout");
            }
            "play" => {
                let raw = parts
                    .next()
                    .and_then(parse_hex_u64)
                    .unwrap_or_else(|| fail("play requires packed move hex"));
                let requested = Move::from_raw(raw);
                let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
                let len = generate_moves(game.position(), &mut moves);
                let Some(mv) = moves[..len].iter().copied().find(|mv| *mv == requested) else {
                    writeln!(stdout, "error illegal_move {:016x}", raw).expect("stdout");
                    stdout.flush().expect("flush");
                    continue;
                };
                game.play(mv);
                writeln!(stdout, "ok").expect("stdout");
            }
            "side" => {
                let side = match game.position().side_to_move {
                    Side::White => "white",
                    Side::Black => "black",
                };
                writeln!(stdout, "side {side}").expect("stdout");
            }
            "status" => write_status(&mut stdout, game.status()),
            "go" => {
                let first = parts.next().unwrap_or("time");
                let limits = match first {
                    "time" => {
                        let time_ms = parts
                            .next()
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(1000);
                        let max_depth = parts
                            .next()
                            .and_then(|s| s.parse::<u8>().ok())
                            .unwrap_or(64);
                        SearchLimits::timed(max_depth, Duration::from_millis(time_ms))
                    }
                    "nodes" => {
                        let nodes = parts
                            .next()
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(100_000);
                        let max_depth = parts
                            .next()
                            .and_then(|s| s.parse::<u8>().ok())
                            .unwrap_or(64);
                        SearchLimits::nodes(max_depth, nodes)
                    }
                    // Backward-compatible v1.1 protocol: go <time_ms> <depth>.
                    legacy => {
                        let time_ms = legacy.parse::<u64>().unwrap_or(1000);
                        let max_depth = parts
                            .next()
                            .and_then(|s| s.parse::<u8>().ok())
                            .unwrap_or(64);
                        SearchLimits::timed(max_depth, Duration::from_millis(time_ms))
                    }
                };
                let result = engine.search(&game, limits);
                match result.decision {
                    ClassicSearchDecision::Move(mv) => {
                        writeln!(
                            stdout,
                            "move {:016x} score {} depth {} nodes {} time_us {}",
                            mv.raw(),
                            result.score,
                            result.depth,
                            result.stats.nodes,
                            result.elapsed.as_micros()
                        )
                        .expect("stdout");
                    }
                    ClassicSearchDecision::ClaimDraw => {
                        writeln!(
                            stdout,
                            "claim score {} depth {} nodes {}",
                            result.score, result.depth, result.stats.nodes
                        )
                        .expect("stdout");
                    }
                    ClassicSearchDecision::ClaimDrawByMove(mv) => {
                        writeln!(
                            stdout,
                            "claim_move {:016x} score {} depth {} nodes {}",
                            mv.raw(),
                            result.score,
                            result.depth,
                            result.stats.nodes
                        )
                        .expect("stdout");
                    }
                    ClassicSearchDecision::Terminal(result) => {
                        write_terminal(&mut stdout, result);
                    }
                    ClassicSearchDecision::NeedsAdjudication(reason) => {
                        writeln!(stdout, "adjudication {reason:?}").expect("stdout");
                    }
                }
            }
            "quit" => {
                writeln!(stdout, "bye").expect("stdout");
                stdout.flush().expect("flush");
                break;
            }
            "version" => {
                writeln!(stdout, "version {ENGINE_VERSION}").expect("stdout");
            }
            _ => {
                writeln!(stdout, "error unknown_command").expect("stdout");
            }
        }
        stdout.flush().expect("flush");
    }
}

fn write_status(out: &mut impl Write, status: GameStatus) {
    match status {
        GameStatus::Ongoing => writeln!(out, "status ongoing").expect("stdout"),
        GameStatus::ClaimableDraw(_) => writeln!(out, "status claimable").expect("stdout"),
        GameStatus::NeedsAdjudication(reason) => {
            writeln!(out, "status adjudication {reason:?}").expect("stdout")
        }
        GameStatus::Finished(result) => write_terminal(out, result),
    }
}

fn write_terminal(out: &mut impl Write, result: GameResult) {
    match result {
        GameResult::Win(Side::White) => writeln!(out, "status white_win").expect("stdout"),
        GameResult::Win(Side::Black) => writeln!(out, "status black_win").expect("stdout"),
        GameResult::Draw(reason) => writeln!(out, "status draw {reason:?}").expect("stdout"),
    }
}

fn parse_hex_u64(text: &str) -> Option<u64> {
    let trimmed = text.strip_prefix("0x").unwrap_or(text);
    u64::from_str_radix(trimmed, 16).ok()
}

fn fail(message: &str) -> ! {
    eprintln!("classic_server: {message}");
    std::process::exit(2);
}
