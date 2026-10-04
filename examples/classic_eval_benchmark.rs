use std::fs;
use std::hint::black_box;
use std::time::Instant;

use russian_draughts_classic::{
    benchmark_suite, coordinates, evaluation_phase, generate_moves, king_attacks_from, square,
    ClassicEngine, ClassicEngineConfig, EvalBreakdown, EvalParams, Game, Move, Position, Side,
    DEFAULT_MOVE_BUFFER_CAPACITY, ENGINE_VERSION,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rounds = args
        .first()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(100_000);
    let eval_path = args.get(1);
    let engine = if let Some(path) = eval_path {
        let text = fs::read_to_string(path).expect("read eval file");
        let eval = EvalParams::parse_kv(&text).expect("parse eval file");
        ClassicEngine::new(ClassicEngineConfig {
            eval,
            ..ClassicEngineConfig::default()
        })
    } else {
        ClassicEngine::default()
    };
    let suite = benchmark_suite();

    for case in &suite {
        black_box(engine.evaluate_score(black_box(&case.game)));
        black_box(engine.evaluate(black_box(&case.game)));
        black_box(scalar_reference_v12(black_box(&case.game)));
    }

    let start = Instant::now();
    let mut fast_checksum = 0i64;
    let mut calls = 0u64;
    for _ in 0..rounds {
        for case in &suite {
            let score = black_box(engine.evaluate_score(black_box(&case.game)));
            fast_checksum = fast_checksum.wrapping_add(score as i64);
            calls += 1;
        }
    }
    let fast_secs = start.elapsed().as_secs_f64();

    let start = Instant::now();
    let mut diagnostic_checksum = 0i64;
    for _ in 0..rounds {
        for case in &suite {
            let eval = black_box(engine.evaluate(black_box(&case.game)));
            diagnostic_checksum = diagnostic_checksum.wrapping_add(eval.side_to_move as i64);
        }
    }
    let diagnostic_secs = start.elapsed().as_secs_f64();

    let start = Instant::now();
    let mut scalar_checksum = 0i64;
    for _ in 0..rounds {
        for case in &suite {
            let eval = black_box(scalar_reference_v12(black_box(&case.game)));
            scalar_checksum = scalar_checksum.wrapping_add(eval.side_to_move as i64);
        }
    }
    let scalar_secs = start.elapsed().as_secs_f64();

    println!("A/B бенчмарк оценочной функции v{ENGINE_VERSION}");
    println!("Позиций в корпусе : {}", suite.len());
    println!("Повторов          : {rounds}");
    println!("Оценок на путь    : {calls}");
    println!(
        "Eval              : {}",
        eval_path.map_or("default", String::as_str)
    );
    println!(
        "Fast tapered      : {:.3} с ({:.0} оценок/с)",
        fast_secs,
        rate(calls, fast_secs)
    );
    println!(
        "Diagnostic tapered: {:.3} с ({:.0} оценок/с)",
        diagnostic_secs,
        rate(calls, diagnostic_secs)
    );
    println!(
        "Historical v1.2   : {:.3} с ({:.0} оценок/с)",
        scalar_secs,
        rate(calls, scalar_secs)
    );
    println!(
        "Fast/diagnostic   : {:.2}x",
        if fast_secs > 0.0 {
            diagnostic_secs / fast_secs
        } else {
            0.0
        }
    );
    println!("Checksum fast     : {fast_checksum}");
    println!("Checksum diagnostic: {diagnostic_checksum}");
    println!("Checksum scalar   : {scalar_checksum}");
    assert_eq!(
        fast_checksum, diagnostic_checksum,
        "fast tapered score must match diagnostic tapered score"
    );
}

fn rate(calls: u64, secs: f64) -> f64 {
    if secs > 0.0 {
        calls as f64 / secs
    } else {
        0.0
    }
}

fn scalar_reference_v12(game: &Game) -> EvalBreakdown {
    const MAN_VALUE: i32 = 100;
    const KING_VALUE: i32 = 290;
    const ADVANCEMENT: [i32; 8] = [0, 2, 6, 12, 21, 33, 52, 0];
    const CENTER_MAN: i32 = 5;
    const CENTER_KING: i32 = 3;
    const INNER_CENTER: i32 = 4;
    const BACK_RANK: i32 = 5;
    const MOBILITY: i32 = 2;
    const KING_MOBILITY: i32 = 1;
    const CONNECTED: i32 = 3;
    const BLOCKED: i32 = 4;
    const PROMOTION: i32 = 10;
    const TEMPO: i32 = 3;

    let pos = game.position();
    let white_kings = pos.white & pos.kings;
    let black_kings = pos.black & pos.kings;
    let white_men = pos.white & !pos.kings;
    let black_men = pos.black & !pos.kings;
    let mut out = EvalBreakdown {
        phase: evaluation_phase(pos) as u16,
        material: (white_men.count_ones() as i32 - black_men.count_ones() as i32) * MAN_VALUE
            + (white_kings.count_ones() as i32 - black_kings.count_ones() as i32) * KING_VALUE,
        advancement: advancement(white_men, Side::White, ADVANCEMENT)
            - advancement(black_men, Side::Black, ADVANCEMENT),
        center: center(
            pos.white,
            white_kings,
            CENTER_MAN,
            CENTER_KING,
            INNER_CENTER,
        ) - center(
            pos.black,
            black_kings,
            CENTER_MAN,
            CENTER_KING,
            INNER_CENTER,
        ),
        ..EvalBreakdown::default()
    };
    let white_back = (white_men & 0x0000_000F).count_ones() as i32;
    let black_back = (black_men & 0xF000_0000).count_ones() as i32;
    let white_scale = black_men.count_ones() as i32;
    let black_scale = white_men.count_ones() as i32;
    out.back_rank = BACK_RANK * (white_back * white_scale - black_back * black_scale) / 12;
    let (white_legal, black_legal) = mobility(pos);
    out.mobility = (white_legal.min(24) as i32 - black_legal.min(24) as i32) * MOBILITY;
    out.king_mobility =
        (king_mobility(pos, white_kings) - king_mobility(pos, black_kings)) * KING_MOBILITY;
    out.structure = (connected(pos.white) - connected(pos.black)) * CONNECTED;
    out.blocked_men =
        (blocked(pos, Side::Black) as i32 - blocked(pos, Side::White) as i32) * BLOCKED;
    out.promotion_pressure = (promotion_pressure(white_men, Side::White)
        - promotion_pressure(black_men, Side::Black))
        * PROMOTION;
    out.tempo = match pos.side_to_move {
        Side::White => TEMPO,
        Side::Black => -TEMPO,
    };
    out.psqt = 0;
    out.total_white = out.material
        + out.advancement
        + out.psqt
        + out.center
        + out.back_rank
        + out.mobility
        + out.king_mobility
        + out.structure
        + out.blocked_men
        + out.promotion_pressure
        + out.tempo;
    out.side_to_move = match pos.side_to_move {
        Side::White => out.total_white,
        Side::Black => -out.total_white,
    };
    out
}

fn advancement(mut men: u32, side: Side, table: [i32; 8]) -> i32 {
    let mut score = 0;
    while men != 0 {
        let sq = pop_lsb(&mut men);
        let rank = sq / 4;
        let progress = match side {
            Side::White => rank,
            Side::Black => 7 - rank,
        } as usize;
        score += table[progress];
    }
    score
}

fn center(mut pieces: u32, kings: u32, man: i32, king: i32, inner: i32) -> i32 {
    let mut score = 0;
    while pieces != 0 {
        let sq = pop_lsb(&mut pieces);
        let (file, rank) = coordinates(sq).expect("valid square");
        if (2..=5).contains(&file) && (2..=5).contains(&rank) {
            score += if kings & bit(sq) != 0 { king } else { man };
            if (3..=4).contains(&file) && (3..=4).contains(&rank) {
                score += inner;
            }
        }
    }
    score
}

fn mobility(pos: Position) -> (usize, usize) {
    let mut white = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let mut black = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    let mut pw = pos;
    pw.side_to_move = Side::White;
    let mut pb = pos;
    pb.side_to_move = Side::Black;
    (
        generate_moves(pw, &mut white),
        generate_moves(pb, &mut black),
    )
}

fn king_mobility(pos: Position, mut kings: u32) -> i32 {
    let occupied = pos.occupied();
    let mut count = 0;
    while kings != 0 {
        let sq = pop_lsb(&mut kings);
        count += (king_attacks_from(sq, occupied) & !occupied).count_ones() as i32;
    }
    count
}

fn connected(pieces: u32) -> i32 {
    let mut p = pieces;
    let mut directed = 0;
    while p != 0 {
        let sq = pop_lsb(&mut p);
        let (file, rank) = coordinates(sq).expect("valid square");
        for (df, dr) in [(-1i8, -1i8), (1, -1), (-1, 1), (1, 1)] {
            let nf = file as i8 + df;
            let nr = rank as i8 + dr;
            if !(0..8).contains(&nf) || !(0..8).contains(&nr) {
                continue;
            }
            if let Some(other) = square(nf as u8, nr as u8) {
                directed += ((pieces & bit(other)) != 0) as i32;
            }
        }
    }
    directed / 2
}

fn blocked(pos: Position, side: Side) -> u32 {
    let ours = match side {
        Side::White => pos.white,
        Side::Black => pos.black,
    };
    let mut men = ours & !pos.kings;
    let occupied = pos.occupied();
    let mut count = 0;
    while men != 0 {
        let sq = pop_lsb(&mut men);
        let (file, rank) = coordinates(sq).expect("valid square");
        let dr = match side {
            Side::White => 1i8,
            Side::Black => -1i8,
        };
        let mut can_step = false;
        for df in [-1i8, 1i8] {
            let nf = file as i8 + df;
            let nr = rank as i8 + dr;
            if !(0..8).contains(&nf) || !(0..8).contains(&nr) {
                continue;
            }
            if let Some(to) = square(nf as u8, nr as u8) {
                if occupied & bit(to) == 0 {
                    can_step = true;
                    break;
                }
            }
        }
        count += (!can_step) as u32;
    }
    count
}

fn promotion_pressure(mut men: u32, side: Side) -> i32 {
    let mut score = 0;
    while men != 0 {
        let sq = pop_lsb(&mut men);
        let rank = sq / 4;
        let progress = match side {
            Side::White => rank,
            Side::Black => 7 - rank,
        };
        score += match progress {
            6 => 2,
            5 => 1,
            _ => 0,
        };
    }
    score
}

#[inline(always)]
fn bit(sq: u8) -> u32 {
    1u32 << sq
}
#[inline(always)]
fn pop_lsb(bb: &mut u32) -> u8 {
    let sq = bb.trailing_zeros() as u8;
    *bb &= *bb - 1;
    sq
}
