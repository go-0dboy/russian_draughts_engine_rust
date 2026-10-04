//! Fixed diagnostic/benchmark corpus. These positions are not claimed to have
//! a known game-theoretic value; they deliberately exercise different rule and
//! search paths so performance changes are not judged on the initial position
//! alone.

use crate::{generate_moves, square, Game, Move, Position, Side, DEFAULT_MOVE_BUFFER_CAPACITY};

#[derive(Clone)]
pub struct BenchmarkCase {
    pub name: &'static str,
    pub category: &'static str,
    pub game: Game,
}

pub fn benchmark_suite() -> Vec<BenchmarkCase> {
    vec![
        BenchmarkCase {
            name: "initial",
            category: "opening",
            game: Game::initial(),
        },
        BenchmarkCase {
            name: "opening-c3-b4",
            category: "opening",
            game: opening_game(&[("c3", "b4")]),
        },
        BenchmarkCase {
            name: "opening-main-line",
            category: "opening",
            game: opening_game(&[("c3", "b4"), ("b6", "a5"), ("g3", "h4")]),
        },
        BenchmarkCase {
            name: "forced-capture",
            category: "capture",
            game: Game::new(position(&["a3", "e3"], &["b4"], &[], Side::White)),
        },
        BenchmarkCase {
            name: "multi-capture",
            category: "capture",
            game: Game::new(position(&["a3"], &["b4", "d6"], &[], Side::White)),
        },
        BenchmarkCase {
            name: "promotion-during-capture",
            category: "promotion",
            game: Game::new(position(&["b6"], &["c7", "f6"], &[], Side::White)),
        },
        BenchmarkCase {
            name: "flying-king-capture",
            category: "kings",
            game: Game::new(position(&["c3"], &["e5"], &["c3"], Side::White)),
        },
        BenchmarkCase {
            name: "king-mobility",
            category: "kings",
            game: Game::new(position(
                &["c3", "g3"],
                &["b6", "h6"],
                &["c3", "b6"],
                Side::White,
            )),
        },
        BenchmarkCase {
            name: "structured-middlegame",
            category: "middlegame",
            game: Game::new(position(
                &["a1", "c3", "e3", "f4", "h4", "b6"],
                &["b8", "d8", "f8", "d6", "g5", "a5"],
                &["a1", "b8"],
                Side::White,
            )),
        },
        BenchmarkCase {
            name: "threefold-claim",
            category: "repetition",
            game: threefold_claim_game(),
        },
        BenchmarkCase {
            name: "three-kings-vs-one",
            category: "endgame",
            game: Game::new(position(
                &["a1", "c3", "e5"],
                &["h8"],
                &["a1", "c3", "e5", "h8"],
                Side::White,
            )),
        },
        BenchmarkCase {
            name: "two-kings",
            category: "endgame",
            game: Game::new(position(&["a1"], &["h8"], &["a1", "h8"], Side::White)),
        },
    ]
}

fn opening_game(line: &[(&str, &str)]) -> Game {
    let mut game = Game::initial();
    for &(from, to) in line {
        let mv = find_move(&game, from, to);
        game.play(mv);
    }
    game
}

fn threefold_claim_game() -> Game {
    let mut game = Game::new(position(
        &["a1", "h6"],
        &["h2", "a7"],
        &["a1", "h2"],
        Side::White,
    ));
    for _ in 0..2 {
        for (from, to) in [("a1", "b2"), ("h2", "g1"), ("b2", "a1"), ("g1", "h2")] {
            let mv = find_move(&game, from, to);
            game.play(mv);
        }
    }
    game
}

fn find_move(game: &Game, from: &str, to: &str) -> Move {
    let mut moves = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = generate_moves(game.position(), &mut moves);
    let from_sq = sq(from);
    let to_sq = sq(to);
    moves[..len]
        .iter()
        .copied()
        .find(|mv| mv.from() == from_sq && mv.to() == to_sq)
        .unwrap_or_else(|| panic!("benchmark move {from}-{to} is not legal"))
}

fn position(white: &[&str], black: &[&str], kings: &[&str], side: Side) -> Position {
    Position {
        white: bb(white),
        black: bb(black),
        kings: bb(kings),
        side_to_move: side,
    }
}

fn bb(names: &[&str]) -> u32 {
    names
        .iter()
        .fold(0u32, |acc, &name| acc | (1u32 << sq(name)))
}

fn sq(name: &str) -> u8 {
    let bytes = name.as_bytes();
    square(bytes[0] - b'a', bytes[1] - b'1').expect("playable draughts square")
}
