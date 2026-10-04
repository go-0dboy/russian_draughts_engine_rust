use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};

use russian_draughts_classic::{
    for_each_pdn_game, generate_moves, read_opening_file, read_tune_file, write_opening_file,
    write_tune_file, DataSplit, Game, GameStatus, Move, OpeningLine, PdnErrorKind, PdnOutcome,
    PositionKey, TuneFileInfo, TuneRecord, DEFAULT_MOVE_BUFFER_CAPACITY, ENGINE_VERSION,
};

#[derive(Debug, Clone)]
struct Options {
    input: PathBuf,
    tune: PathBuf,
    openings: PathBuf,
    errors: PathBuf,
    tune_min_ply: usize,
    tune_stride: usize,
    tune_per_game: usize,
    opening_count: usize,
    opening_min_ply: usize,
    opening_max_ply: usize,
    seed: u64,
    include_fen_tuning: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CorpusRole {
    Train,
    Validation,
    Test,
    Arena,
}

impl CorpusRole {
    const fn tune_split(self) -> Option<DataSplit> {
        match self {
            Self::Train => Some(DataSplit::Train),
            Self::Validation => Some(DataSplit::Validation),
            Self::Test => Some(DataSplit::Test),
            Self::Arena => None,
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            input: PathBuf::new(),
            tune: PathBuf::from("data/corpus/tuning.rdtune"),
            openings: PathBuf::from("data/corpus/openings.rdopen"),
            errors: PathBuf::from("data/corpus/rejected.tsv"),
            tune_min_ply: 10,
            tune_stride: 3,
            tune_per_game: 8,
            opening_count: 4096,
            opening_min_ply: 6,
            opening_max_ply: 14,
            seed: 0x2026_0925_DA7A_0001,
            include_fen_tuning: false,
        }
    }
}

fn main() {
    if let Err(err) = real_main() {
        eprintln!("classic_corpus: {err}");
        std::process::exit(2);
    }
}

fn real_main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else {
        usage();
        return Ok(());
    };
    match cmd.as_str() {
        "compile" => compile(parse_compile_args(args.collect())?),
        "inspect" => {
            let path = args.next().ok_or("inspect requires a file")?;
            inspect(Path::new(&path))
        }
        "--help" | "-h" | "help" => {
            usage();
            Ok(())
        }
        _ => Err(format!("unknown command {cmd}").into()),
    }
}

fn usage() {
    println!(
        "Russian draughts corpus compiler v{ENGINE_VERSION}\n\
usage:\n\
  classic_corpus compile russian2012.pdn [options]\n\
  classic_corpus inspect data/corpus/tuning.rdtune\n\n\
options:\n\
  --tune FILE                 output tuning corpus\n\
  --openings FILE             output paired-match opening suite\n\
  --errors FILE               rejected-game report\n\
  --tune-min-ply N            default 10\n\
  --tune-stride N             candidate spacing, default 3\n\
  --tune-per-game N           max quiet positions/game, default 8\n\
  --opening-count N           reservoir size, default 4096\n\
  --opening-min-ply N         default 6\n\
  --opening-max-ply N         default 14\n\
  --seed N                    deterministic decimal/0x seed\n\
  --include-fen-tuning        include standalone FEN games in tune data"
    );
}

fn parse_compile_args(args: Vec<String>) -> Result<Options, Box<dyn std::error::Error>> {
    if args.is_empty() {
        return Err("compile requires input PDN".into());
    }
    let mut o = Options {
        input: PathBuf::from(&args[0]),
        ..Options::default()
    };
    let mut i = 1;
    while i < args.len() {
        let key = &args[i];
        match key.as_str() {
            "--include-fen-tuning" => {
                o.include_fen_tuning = true;
                i += 1;
            }
            "--tune" | "--openings" | "--errors" | "--tune-min-ply" | "--tune-stride"
            | "--tune-per-game" | "--opening-count" | "--opening-min-ply" | "--opening-max-ply"
            | "--seed" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{key} requires a value"))?;
                match key.as_str() {
                    "--tune" => o.tune = PathBuf::from(value),
                    "--openings" => o.openings = PathBuf::from(value),
                    "--errors" => o.errors = PathBuf::from(value),
                    "--tune-min-ply" => o.tune_min_ply = value.parse()?,
                    "--tune-stride" => o.tune_stride = value.parse()?,
                    "--tune-per-game" => o.tune_per_game = value.parse()?,
                    "--opening-count" => o.opening_count = value.parse()?,
                    "--opening-min-ply" => o.opening_min_ply = value.parse()?,
                    "--opening-max-ply" => o.opening_max_ply = value.parse()?,
                    "--seed" => o.seed = parse_u64(value)?,
                    _ => unreachable!(),
                }
                i += 2;
            }
            _ => return Err(format!("unknown option {key}").into()),
        }
    }
    if o.tune_stride == 0 || o.tune_per_game == 0 {
        return Err("tune stride/per-game must be > 0".into());
    }
    if o.opening_min_ply == 0 || o.opening_max_ply < o.opening_min_ply {
        return Err("invalid opening ply range".into());
    }
    Ok(o)
}

fn compile(o: Options) -> Result<(), Box<dyn std::error::Error>> {
    create_parent(&o.tune)?;
    create_parent(&o.openings)?;
    create_parent(&o.errors)?;

    let file = File::open(&o.input)?;
    let reader = BufReader::with_capacity(1 << 20, file);
    let mut tune_records = Vec::<TuneRecord>::new();
    let mut openings = Vec::<OpeningLine>::new();
    let mut opening_seen = HashSet::<u128>::new();
    let mut opening_candidates = 0u64;
    let mut rng = SplitMix64::new(o.seed);
    let mut errors = Vec::<String>::new();
    let mut reasons = HashMap::<PdnErrorKind, u64>::new();
    let mut accepted_games = 0u64;
    let mut completed_games = 0u64;
    let mut fen_games = 0u64;
    let mut processed_games = 0u64;
    let mut role_games = [0u64; 4];

    let stream_stats = for_each_pdn_game(reader, |result| {
        processed_games += 1;
        if processed_games % 10_000 == 0 {
            eprintln!("processed {processed_games} games...");
        }
        match result {
            Ok(pdn) => {
                accepted_games += 1;
                if pdn.starts_from_fen {
                    fen_games += 1;
                }
                if pdn.outcome != PdnOutcome::Unknown {
                    completed_games += 1;
                }

                let game_key = game_fingerprint(pdn.start_position, &pdn.moves);
                let role = role_for_game(game_key, o.seed);
                role_games[match role {
                    CorpusRole::Train => 0,
                    CorpusRole::Validation => 1,
                    CorpusRole::Test => 2,
                    CorpusRole::Arena => 3,
                }] += 1;

                // Match/search openings come only from the arena role. These
                // games never contribute HCE tuning positions, preventing
                // development matches from reusing tune/test games.
                if role == CorpusRole::Arena
                    && !pdn.starts_from_fen
                    && pdn.outcome != PdnOutcome::Unknown
                {
                    maybe_add_opening(
                        &pdn.moves,
                        pdn.index,
                        &o,
                        &mut openings,
                        &mut opening_seen,
                        &mut opening_candidates,
                        &mut rng,
                    );
                }

                if let Some(split) = role.tune_split() {
                    if pdn.outcome != PdnOutcome::Unknown
                        && (o.include_fen_tuning || !pdn.starts_from_fen)
                    {
                        collect_tune_positions(
                            pdn.start_position,
                            &pdn.moves,
                            pdn.outcome,
                            split,
                            &o,
                            &mut tune_records,
                        );
                    }
                }
            }
            Err(err) => {
                *reasons.entry(err.kind.clone()).or_insert(0) += 1;
                errors.push(format!(
                    "{}\t{}\t{:?}\t{}",
                    err.game_index,
                    err.ply,
                    err.kind,
                    err.message.replace(['\t', '\n'], " ")
                ));
            }
        }
    })?;

    let leakage_removed = remove_cross_split_position_leakage(&mut tune_records);
    let tune_keys: HashSet<u128> = tune_records
        .iter()
        .map(|record| PositionKey::from_position(record.position).0)
        .collect();
    let before_arena_filter = openings.len();
    openings.retain(|opening| !opening_intersects_positions(opening, &tune_keys));
    let arena_overlap_drops = before_arena_filter - openings.len();

    let info = TuneFileInfo {
        records: tune_records.len() as u64,
        source_games: stream_stats.games_seen,
        accepted_games,
        rejected_games: stream_stats.games_rejected,
    };
    write_tune_file(&o.tune, &tune_records, info)?;
    write_opening_file(&o.openings, &openings)?;

    let mut rejected = File::create(&o.errors)?;
    writeln!(rejected, "game\tply\treason\tmessage")?;
    for line in &errors {
        writeln!(rejected, "{line}")?;
    }

    let mut train = 0usize;
    let mut validation = 0usize;
    let mut test = 0usize;
    for r in &tune_records {
        match r.split {
            DataSplit::Train => train += 1,
            DataSplit::Validation => validation += 1,
            DataSplit::Test => test += 1,
        }
    }

    println!("Corpus compiler v{ENGINE_VERSION}");
    println!("input             : {}", o.input.display());
    println!("games seen        : {}", stream_stats.games_seen);
    println!("games accepted    : {accepted_games}");
    println!("games rejected    : {}", stream_stats.games_rejected);
    println!("completed games   : {completed_games}");
    println!("FEN starts        : {fen_games}");
    println!(
        "game roles        : train {} validation {} test {} arena {}",
        role_games[0], role_games[1], role_games[2], role_games[3]
    );
    println!("cross-split drops : {leakage_removed}");
    println!("arena overlap drops: {arena_overlap_drops}");
    println!(
        "tune positions    : {} (train {train}, validation {validation}, test {test})",
        tune_records.len()
    );
    println!("opening candidates: {opening_candidates}");
    println!("opening suite     : {}", openings.len());
    println!("tune file         : {}", o.tune.display());
    println!("opening file      : {}", o.openings.display());
    println!("reject report     : {}", o.errors.display());
    if !reasons.is_empty() {
        println!("rejections by reason:");
        let mut rows: Vec<_> = reasons.into_iter().collect();
        rows.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        for (kind, count) in rows {
            println!("  {kind:?}: {count}");
        }
    }
    Ok(())
}

fn collect_tune_positions(
    start: russian_draughts_classic::Position,
    moves: &[Move],
    outcome: PdnOutcome,
    split: DataSplit,
    o: &Options,
    output: &mut Vec<TuneRecord>,
) {
    let mut game = Game::new(start);
    let mut candidates = Vec::<(u64, TuneRecord)>::new();
    let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];

    for (ply, &mv) in moves.iter().enumerate() {
        if ply >= o.tune_min_ply && (ply - o.tune_min_ply) % o.tune_stride == 0 {
            let len = generate_moves(game.position(), &mut legal);
            let quiet = len > 0 && !legal[0].is_capture();
            let ongoing = matches!(
                game.status_with_legal_count(len),
                GameStatus::Ongoing | GameStatus::ClaimableDraw(_)
            );
            if quiet && ongoing {
                let key = PositionKey::from_position(game.position()).0;
                let hash = mix64(key as u64 ^ (key >> 64) as u64 ^ o.seed);
                candidates.push((
                    hash,
                    TuneRecord {
                        position: game.position(),
                        outcome,
                        split,
                    },
                ));
            }
        }
        game.play(mv);
    }

    // Deduplicate repetitions only inside this game.  Do not globally keep
    // the first occurrence of a board: the same position reached in different
    // tournament games carries independent W/D/L evidence and discarding all
    // later results would bias the tuner toward file order.
    candidates.sort_unstable_by_key(|(hash, _)| *hash);
    let mut local_seen = HashSet::<u128>::new();
    for (_, record) in candidates {
        let key = PositionKey::from_position(record.position).0;
        if !local_seen.insert(key) {
            continue;
        }
        output.push(record);
        if local_seen.len() >= o.tune_per_game {
            break;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn maybe_add_opening(
    moves: &[Move],
    game_index: usize,
    o: &Options,
    openings: &mut Vec<OpeningLine>,
    seen: &mut HashSet<u128>,
    candidates_seen: &mut u64,
    rng: &mut SplitMix64,
) {
    if moves.len() <= o.opening_min_ply {
        return;
    }
    let span = o.opening_max_ply - o.opening_min_ply + 1;
    let desired = o.opening_min_ply + (mix64(game_index as u64 ^ o.seed) as usize % span);
    let len = desired.min(moves.len().saturating_sub(1));
    if len == 0 {
        return;
    }
    let line = &moves[..len];

    // Deduplicate by the resulting board, not by textual move sequence.
    // Transpositions are therefore represented only once in match tests.
    let mut game = Game::initial();
    for &mv in line {
        game.play(mv);
    }
    if !matches!(game.status(), GameStatus::Ongoing) {
        return;
    }
    let endpoint = PositionKey::from_position(game.position()).0;
    if !seen.insert(endpoint) {
        return;
    }

    *candidates_seen += 1;
    let opening = OpeningLine {
        moves: line.to_vec(),
    };
    if openings.len() < o.opening_count {
        openings.push(opening);
        return;
    }
    if o.opening_count == 0 {
        return;
    }
    let slot = (rng.next_u64() % *candidates_seen) as usize;
    if slot < o.opening_count {
        openings[slot] = opening;
    }
}

fn opening_intersects_positions(opening: &OpeningLine, protected: &HashSet<u128>) -> bool {
    let mut game = Game::initial();
    for &mv in &opening.moves {
        game.play(mv);
        if protected.contains(&PositionKey::from_position(game.position()).0) {
            return true;
        }
    }
    false
}

/// Stable semantic game identity for corpus partitioning. It deliberately does
/// not depend on PDN file order or packed move metadata, so inserting/reordering
/// unrelated games cannot move an existing game between train/validation/test/arena.
fn game_fingerprint(start: russian_draughts_classic::Position, moves: &[Move]) -> u64 {
    let key = PositionKey::from_position(start).0;
    let mut h = mix64(key as u64 ^ (key >> 64) as u64 ^ 0x5244_4347_414d_4531);
    for &mv in moves {
        let semantic = mv.captures() as u64 ^ ((mv.from() as u64) << 32) ^ ((mv.to() as u64) << 37);
        h = mix64(h ^ semantic ^ 0x9e37_79b9_7f4a_7c15);
    }
    mix64(h ^ moves.len() as u64)
}

fn role_for_game(game_fingerprint: u64, seed: u64) -> CorpusRole {
    match mix64(game_fingerprint ^ seed) % 100 {
        0..=79 => CorpusRole::Train,
        80..=89 => CorpusRole::Validation,
        90..=94 => CorpusRole::Test,
        _ => CorpusRole::Arena,
    }
}

/// Preserve the game-level split while preventing exact board identity from
/// leaking into a protected holdout. If a position occurs in multiple tuning
/// roles, keep it only in the most protected role: test > validation > train.
fn remove_cross_split_position_leakage(records: &mut Vec<TuneRecord>) -> usize {
    let mut strongest = HashMap::<u128, u8>::new();
    for record in records.iter() {
        let key = PositionKey::from_position(record.position).0;
        let rank = match record.split {
            DataSplit::Test => 0,
            DataSplit::Validation => 1,
            DataSplit::Train => 2,
        };
        strongest
            .entry(key)
            .and_modify(|existing| *existing = (*existing).min(rank))
            .or_insert(rank);
    }
    let before = records.len();
    records.retain(|record| {
        let key = PositionKey::from_position(record.position).0;
        let rank = match record.split {
            DataSplit::Test => 0,
            DataSplit::Validation => 1,
            DataSplit::Train => 2,
        };
        strongest.get(&key).copied() == Some(rank)
    });
    before - records.len()
}

fn inspect(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match path.extension().and_then(|x| x.to_str()) {
        Some("rdtune") => {
            let (info, records) = read_tune_file(path)?;
            let mut split = [0u64; 3];
            let mut result = [0u64; 4];
            for r in records {
                split[r.split as usize] += 1;
                result[r.outcome.code() as usize] += 1;
            }
            println!("RDTUNE2 {}", path.display());
            println!("records       : {}", info.records);
            println!("source games  : {}", info.source_games);
            println!("accepted games: {}", info.accepted_games);
            println!("rejected games: {}", info.rejected_games);
            println!("split T/V/test: {}/{}/{}", split[0], split[1], split[2]);
            println!(
                "result B/D/W/*: {}/{}/{}/{}",
                result[0], result[1], result[2], result[3]
            );
        }
        Some("rdopen") => {
            let openings = read_opening_file(path)?;
            let plies: usize = openings.iter().map(|o| o.moves.len()).sum();
            println!("RDOPEN2 {}", path.display());
            println!("openings: {}", openings.len());
            println!("plies   : {plies}");
            if !openings.is_empty() {
                println!("average : {:.2}", plies as f64 / openings.len() as f64);
            }
        }
        _ => return Err("inspect expects .rdtune or .rdopen".into()),
    }
    Ok(())
}

fn create_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

fn parse_u64(text: &str) -> Result<u64, Box<dyn std::error::Error>> {
    Ok(if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)?
    } else {
        text.parse()?
    })
}

#[inline(always)]
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

struct SplitMix64(u64);
impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix64(self.0)
    }
}

#[cfg(test)]
mod corpus_split_tests {
    use super::*;
    use russian_draughts_classic::{Position, Side};

    #[test]
    fn role_split_is_deterministic_and_reserves_arena() {
        let seed = 0x1234_5678;
        let roles: Vec<_> = (0..10_000).map(|key| role_for_game(key, seed)).collect();
        assert!(roles.contains(&CorpusRole::Arena));
        assert_eq!(
            roles,
            (0..10_000)
                .map(|key| role_for_game(key, seed))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn game_fingerprint_depends_on_semantics_not_source_index() {
        let start = Position::initial();
        let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let len = generate_moves(start, &mut legal);
        assert!(len >= 2);
        let a = game_fingerprint(start, &[legal[0]]);
        let b = game_fingerprint(start, &[legal[0]]);
        let c = game_fingerprint(start, &[legal[1]]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(role_for_game(a, 7), role_for_game(b, 7));
    }

    #[test]
    fn exact_position_does_not_cross_tune_splits() {
        let p = Position::initial();
        let q = Position {
            side_to_move: Side::Black,
            ..p
        };
        let mut records = vec![
            TuneRecord {
                position: p,
                outcome: PdnOutcome::Draw,
                split: DataSplit::Test,
            },
            TuneRecord {
                position: p,
                outcome: PdnOutcome::WhiteWin,
                split: DataSplit::Train,
            },
            TuneRecord {
                position: q,
                outcome: PdnOutcome::Draw,
                split: DataSplit::Test,
            },
            TuneRecord {
                position: q,
                outcome: PdnOutcome::BlackWin,
                split: DataSplit::Validation,
            },
        ];
        assert_eq!(remove_cross_split_position_leakage(&mut records), 2);
        assert!(records
            .iter()
            .any(|r| r.position == p && r.split == DataSplit::Test));
        assert!(records
            .iter()
            .any(|r| r.position == q && r.split == DataSplit::Test));
        assert!(!records.iter().any(|r| r.split == DataSplit::Train));
        assert!(!records.iter().any(|r| r.split == DataSplit::Validation));
    }
    #[test]
    fn arena_opening_is_rejected_if_any_position_hits_tuning_holdout() {
        let mut legal = [Move::EMPTY; DEFAULT_MOVE_BUFFER_CAPACITY];
        let start = Position::initial();
        let len = generate_moves(start, &mut legal);
        assert!(len > 0);
        let line = OpeningLine {
            moves: vec![legal[0]],
        };
        let mut game = Game::initial();
        game.play(legal[0]);
        let protected = HashSet::from([PositionKey::from_position(game.position()).0]);
        assert!(opening_intersects_positions(&line, &protected));
        assert!(!opening_intersects_positions(&line, &HashSet::new()));
    }
}
