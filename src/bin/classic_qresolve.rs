//! Offline capture-only minimax resolver. No TT, pruning or mutable evaluation.
//! Full game history is replayed. A budget/inexact adjudication invalidates the sample.
use russian_draughts_classic::{
    generate_moves, EvalFeatures, EvalParams, Evaluator, Game, GameResult, GameStatus, Move,
    Position, Side, DEFAULT_MOVE_BUFFER_CAPACITY,
};
use std::io::{self, BufRead, Write};

struct Answer {
    score: i32,
    leaf: Option<(Position, EvalFeatures)>,
    path: Vec<u64>,
}
fn resolve(
    game: &mut Game,
    eval: &Evaluator,
    nodes: &mut u64,
    limit: u64,
    ply: usize,
) -> Result<Answer, &'static str> {
    if *nodes >= limit {
        return Err("node_limit");
    }
    *nodes += 1;
    if ply >= 64 || game.history().remaining_capacity() == 0 {
        return Err("history_or_depth_limit");
    }
    let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = generate_moves(game.position(), &mut moves);
    let status = game.status_with_legal_count(len);
    match status {
        GameStatus::Finished(result) => {
            let score = match result {
                GameResult::Win(side) => {
                    if side == game.position().side_to_move {
                        30000 - ply as i32
                    } else {
                        -30000 + ply as i32
                    }
                }
                GameResult::Draw(_) => 0,
            };
            return Ok(Answer {
                score,
                leaf: None,
                path: Vec::new(),
            });
        }
        GameStatus::NeedsAdjudication(_) => return Err("adjudication"),
        _ => {}
    }
    let capture = len > 0 && moves[0].is_capture();
    let claim = matches!(status, GameStatus::ClaimableDraw(_))
        || (!capture
            && moves[..len]
                .iter()
                .any(|&mv| game.move_creates_third_repetition(mv)));
    if !capture {
        let score = eval.score(game);
        // Draw ties use the draw action; no static feature vector represents it.
        return Ok(if claim && score <= 0 {
            Answer {
                score: 0,
                leaf: None,
                path: Vec::new(),
            }
        } else {
            Answer {
                score,
                leaf: Some((game.position(), eval.features(game))),
                path: Vec::new(),
            }
        });
    }
    let mut best = Answer {
        score: if claim { 0 } else { -32000 },
        leaf: None,
        path: Vec::new(),
    };
    for &mv in &moves[..len] {
        let undo = game.play_with_undo(mv);
        let child = resolve(game, eval, nodes, limit, ply + 1);
        game.undo(undo);
        let mut child = child?;
        let score = -child.score;
        if score > best.score {
            child.path.insert(0, mv.raw());
            best = Answer {
                score,
                leaf: child.leaf,
                path: child.path,
            };
        }
    }
    Ok(best)
}
fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: classic_qresolve EVAL_FILE NODE_LIMIT".into());
    }
    let params = EvalParams::parse_kv(&std::fs::read_to_string(&args[0])?)?;
    let eval = Evaluator::new(params);
    let limit: u64 = args[1].parse()?;
    if limit == 0 {
        return Err("node limit must be positive".into());
    }
    let mut out = io::BufWriter::new(io::stdout().lock());
    writeln!(out, "RDQ1")?;
    out.flush()?;
    for line in io::stdin().lock().lines() {
        let line = line?;
        let t: Vec<_> = line.split_whitespace().collect();
        if t.len() != 7 || t[0] != "G" {
            return Err("bad request".into());
        }
        let pos = Position {
            white: t[1].parse()?,
            black: t[2].parse()?,
            kings: t[3].parse()?,
            side_to_move: match t[4] {
                "0" => Side::White,
                "1" => Side::Black,
                _ => return Err("bad side".into()),
            },
        };
        russian_draughts_classic::api::validate_position(pos)?;
        let requested: Vec<usize> = t[5].split(',').map(str::parse).collect::<Result<_, _>>()?;
        let replay: Vec<u64> = if t[6] == "-" {
            Vec::new()
        } else {
            t[6].split(',')
                .map(|s| u64::from_str_radix(s, 16))
                .collect::<Result<_, _>>()?
        };
        if requested.windows(2).any(|w| w[0] >= w[1])
            || requested.last().map_or(true, |&p| p >= replay.len())
        {
            return Err("invalid requested plies".into());
        }
        let mut game = Game::new(pos);
        let mut next = 0;
        for (ply, &raw) in replay.iter().enumerate() {
            if next < requested.len() && requested[next] == ply {
                let mut nodes = 0;
                match resolve(&mut game, &eval, &mut nodes, limit, 0) {
                    Err(reason) => writeln!(out, "X\t{ply}\t{reason}\t{nodes}")?,
                    Ok(a) => {
                        if let Some((p, f)) = a.leaf {
                            write!(
                                out,
                                "Q\t{ply}\t{}\t{nodes}\t{}\t{}\t{}\t{}\t{}\t",
                                a.score, p.white, p.black, p.kings, p.side_to_move as u8, f.phase
                            )?;
                            for (i, v) in f.values.iter().enumerate() {
                                if i > 0 {
                                    write!(out, ",")?;
                                }
                                write!(out, "{v}")?;
                            }
                            write!(out, "\t")?;
                            if a.path.is_empty() {
                                write!(out, "-")?;
                            }
                            for (i, mv) in a.path.iter().enumerate() {
                                if i > 0 {
                                    write!(out, ",")?;
                                }
                                write!(out, "{mv:016x}")?;
                            }
                            let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
                            let n = generate_moves(p, &mut legal);
                            let flags = if legal[..n].iter().any(|m| m.promotes()) {
                                2
                            } else {
                                0
                            };
                            writeln!(out, "\t{flags}")?;
                        } else {
                            writeln!(out, "X\t{ply}\tterminal_or_claim\t{nodes}")?;
                        }
                    }
                }
                next += 1;
            }
            if next == requested.len() {
                break;
            }
            let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
            let len = generate_moves(game.position(), &mut moves);
            let mv = moves[..len]
                .iter()
                .find(|m| m.raw() == raw)
                .ok_or("illegal replay")?;
            if !matches!(
                game.status_with_legal_count(len),
                GameStatus::Ongoing | GameStatus::ClaimableDraw(_)
            ) {
                return Err("replay after terminal".into());
            }
            game.play(*mv);
        }
        writeln!(out, "Z")?;
        out.flush()?;
    }
    Ok(())
}
fn main() {
    if let Err(err) = serve() {
        eprintln!("classic_qresolve: {err}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_leaf_is_exact_and_preserves_root() {
        let mut g = Game::new(Position::initial());
        let p = g.position();
        let e = Evaluator::default();
        let a = resolve(&mut g, &e, &mut 0, 1000, 0).unwrap();
        assert_eq!(a.score, e.score(&g));
        assert_eq!(a.leaf.unwrap().0, p);
        assert!(a.path.is_empty());
        assert_eq!(g.position(), p);
    }
    fn board(white: &[&str], black: &[&str]) -> Position {
        let bits = |squares: &[&str]| {
            squares.iter().fold(0, |b, s| {
                let bytes = s.as_bytes();
                b | (1
                    << russian_draughts_classic::square(bytes[0] - b'a', bytes[1] - b'1').unwrap())
            })
        };
        Position {
            white: bits(white),
            black: bits(black),
            kings: 0,
            side_to_move: Side::White,
        }
    }
    #[test]
    fn mandatory_capture_returns_quiet_leaf_with_correct_sign() {
        let p = board(&["a3", "e3"], &["b4", "h8"]);
        let mut g = Game::new(p);
        let e = Evaluator::default();
        let a = resolve(&mut g, &e, &mut 0, 1000, 0).unwrap();
        let (leaf, features) = a.leaf.unwrap();
        assert_eq!(a.path.len(), 1);
        assert_eq!(a.score, features.score_white(EvalParams::default()));
        assert_eq!(leaf.side_to_move, Side::Black);
        assert_eq!(g.position(), p);
        let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let n = generate_moves(leaf, &mut legal);
        assert!(n > 0);
        assert!(!legal[0].is_capture());
        assert!(resolve(&mut g, &e, &mut 0, 1, 0).is_err());
        assert_eq!(g.position(), p);
    }
    #[test]
    fn promotion_capture_is_resolved_by_real_move_generator() {
        let p = board(&["b6"], &["c7", "f6", "h8"]);
        let mut g = Game::new(p);
        let a = resolve(&mut g, &Evaluator::default(), &mut 0, 10000, 0).unwrap();
        if let Some((leaf, _)) = a.leaf {
            assert_ne!(leaf.white & leaf.kings, 0);
        }
        assert!(!a.path.is_empty());
        assert_eq!(g.position(), p);
    }
    #[test]
    fn terminal_win_has_no_trainable_static_leaf() {
        let mut g = Game::new(board(&["a3"], &["b4"]));
        let a = resolve(&mut g, &Evaluator::default(), &mut 0, 1000, 0).unwrap();
        assert!(a.score > 29000);
        assert!(a.leaf.is_none());
    }
    #[test]
    fn exhausted_budget_is_not_a_leaf() {
        assert!(resolve(
            &mut Game::new(Position::initial()),
            &Evaluator::default(),
            &mut 0,
            0,
            0
        )
        .is_err());
    }
}
