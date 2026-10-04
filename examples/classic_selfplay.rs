use russian_draughts_classic::{
    coordinates, ClassicEngine, ClassicSearchDecision, Game, GameStatus, SearchLimits,
    ENGINE_VERSION,
};

fn main() {
    let depth = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(8);
    let max_plies = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(200);

    let mut white = ClassicEngine::default();
    let mut black = ClassicEngine::default();
    let mut game = Game::initial();

    println!("Самоигра классического движка v{ENGINE_VERSION}");
    println!("Глубина: {depth}, максимум полуходов: {max_plies}");

    for ply in 0..max_plies {
        match game.status() {
            GameStatus::Finished(result) => {
                println!("Результат: {result:?}, полуходов: {ply}");
                return;
            }
            GameStatus::NeedsAdjudication(reason) => {
                println!("Требуется внешний арбитр/tablebase: {reason:?}, полуходов: {ply}");
                return;
            }
            GameStatus::Ongoing | GameStatus::ClaimableDraw(_) => {}
        }

        let engine = match game.position().side_to_move {
            russian_draughts_classic::Side::White => &mut white,
            russian_draughts_classic::Side::Black => &mut black,
        };
        let result = engine.search(&game, SearchLimits::depth(depth));
        match result.decision {
            ClassicSearchDecision::Move(mv) => {
                println!(
                    "{:03}. {:?}: {}  score={} depth={} nodes={}",
                    ply + 1,
                    game.position().side_to_move,
                    move_text(mv),
                    result.score,
                    result.depth,
                    result.stats.nodes,
                );
                game.play(mv);
            }
            ClassicSearchDecision::ClaimDraw | ClassicSearchDecision::ClaimDrawByMove(_) => {
                println!("Ничья заявлена на полуходе {}", ply + 1);
                return;
            }
            ClassicSearchDecision::Terminal(result) => {
                println!("Результат: {result:?}");
                return;
            }
            ClassicSearchDecision::NeedsAdjudication(reason) => {
                println!("Требуется внешний арбитр/tablebase: {reason:?}");
                return;
            }
        }
    }
    println!("Достигнут внешний лимит {max_plies} полуходов без искусственного присвоения ничьей.");
}

fn move_text(mv: russian_draughts_classic::Move) -> String {
    let (ff, fr) = coordinates(mv.from()).unwrap();
    let (tf, tr) = coordinates(mv.to()).unwrap();
    let sep = if mv.is_capture() { ':' } else { '-' };
    format!(
        "{}{}{}{}{}",
        (b'a' + ff) as char,
        fr + 1,
        sep,
        (b'a' + tf) as char,
        tr + 1
    )
}
