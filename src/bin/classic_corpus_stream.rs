//! Trusted offline bridge: existing PDN parser and exact HCE feature extractor.
//! stdout is a versioned TSV stream consumed by tools/corpus_v2.py.
use russian_draughts_classic::{
    for_each_pdn_game, generate_moves, Evaluator, Game, GameStatus, Move,
    DEFAULT_MOVE_BUFFER_CAPACITY,
};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};

fn emit(out: &mut impl Write, pdn: russian_draughts_classic::PdnGame) -> io::Result<()> {
    if pdn.outcome.white_score().is_none() {
        return Ok(());
    }
    let p = pdn.start_position;
    write!(
        out,
        "G\t{}\t{}\t{}\t{}\t{}\t{}\t",
        pdn.index,
        p.white,
        p.black,
        p.kings,
        p.side_to_move as u8,
        pdn.outcome.code()
    )?;
    for (i, mv) in pdn.moves.iter().enumerate() {
        if i > 0 {
            write!(out, ";")?;
        }
        write!(out, "{}:{}:{}", mv.from(), mv.to(), mv.captures())?;
    }
    write!(out, "\t")?;
    for (i, mv) in pdn.moves.iter().enumerate() {
        if i > 0 {
            write!(out, ",")?;
        }
        write!(out, "{:016x}", mv.raw())?;
    }
    writeln!(out)?;
    let evaluator = Evaluator::default();
    let mut game = Game::new(p);
    let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
    for (ply, mv) in pdn.moves.iter().enumerate() {
        let len = generate_moves(game.position(), &mut legal);
        if !matches!(
            game.status_with_legal_count(len),
            GameStatus::Ongoing | GameStatus::ClaimableDraw(_)
        ) {
            break;
        }
        let p = game.position();
        let features = evaluator.features(&game);
        let capture = legal[..len].iter().any(|m| m.is_capture());
        let promotion = legal[..len].iter().any(|m| m.promotes());
        write!(
            out,
            "S\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t",
            ply,
            p.white,
            p.black,
            p.kings,
            p.side_to_move as u8,
            features.phase,
            u8::from(capture),
            u8::from(promotion)
        )?;
        for (i, v) in features.values.iter().enumerate() {
            if i > 0 {
                write!(out, ",")?;
            }
            write!(out, "{v}")?;
        }
        writeln!(out)?;
        game.play(*mv);
    }
    Ok(())
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 1 {
        return Err("usage: classic_corpus_stream SOURCE.pdn".into());
    }
    let mut out = BufWriter::new(io::stdout().lock());
    writeln!(out, "RDSTREAM1")?;
    let mut failure = None;
    let stats = for_each_pdn_game(BufReader::new(File::open(&args[0])?), |result| {
        if failure.is_some() {
            return;
        }
        match result {
            Ok(pdn) => {
                if let Err(err) = emit(&mut out, pdn) {
                    failure = Some(err);
                }
            }
            Err(err) => {
                if let Err(e) = writeln!(out, "E\t{}\t{}\t{:?}", err.game_index, err.ply, err.kind)
                {
                    failure = Some(e);
                }
            }
        }
    })?;
    if let Some(err) = failure {
        return Err(err.into());
    }
    writeln!(out, "Z\t{}\t{}", stats.games_seen, stats.games_rejected)?;
    out.flush()?;
    Ok(())
}
fn main() {
    if let Err(err) = run() {
        eprintln!("classic_corpus_stream: {err}");
        std::process::exit(2);
    }
}
