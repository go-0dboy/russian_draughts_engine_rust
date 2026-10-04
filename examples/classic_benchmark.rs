use russian_draughts_classic::{
    ClassicEngine, ClassicEngineConfig, Game, SearchLimits, ENGINE_VERSION,
};

fn main() {
    let depth = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(10);
    let rounds = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(3);
    let tt_mb = std::env::args()
        .nth(3)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(64);

    println!("Бенчмарк классического поиска русских шашек v{ENGINE_VERSION}");
    println!("Глубина      : {depth}");
    println!("Повторов     : {rounds}");
    println!("TT           : {tt_mb} МБ");

    let game = Game::initial();
    let mut total_nodes = 0u64;
    let mut total_secs = 0.0f64;

    for round in 0..rounds {
        let mut engine = ClassicEngine::new(ClassicEngineConfig {
            tt_megabytes: tt_mb,
            ..ClassicEngineConfig::default()
        });
        let result = engine.search(&game, SearchLimits::depth(depth));
        let secs = result.elapsed.as_secs_f64();
        total_nodes += result.stats.nodes;
        total_secs += secs;
        println!(
            "#{:02}: score={:>6} depth={:>2} nodes={:>12} time={:>8.3}s nps={:>12.0} tt={}/{}",
            round + 1,
            result.score,
            result.depth,
            result.stats.nodes,
            secs,
            result.nodes_per_second(),
            result.stats.tt_hits,
            result.stats.tt_probes,
        );
    }

    println!("Итого узлов  : {total_nodes}");
    println!("Итого времени: {:.3} с", total_secs);
    println!(
        "Среднее nps  : {:.0}",
        if total_secs > 0.0 {
            total_nodes as f64 / total_secs
        } else {
            0.0
        }
    );
}
