use russian_draughts_classic::{
    benchmark_suite, ClassicEngine, ClassicEngineConfig, SearchLimits, ENGINE_VERSION,
};

fn main() {
    let depth = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(10);
    let tt_mb = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(16);

    let suite = benchmark_suite();
    println!("Корпусный бенчмарк классического движка v{ENGINE_VERSION}");
    println!("Глубина : {depth}");
    println!("TT      : {tt_mb} МБ");
    println!("Позиций : {}", suite.len());
    println!();

    let mut total_nodes = 0u64;
    let mut total_qnodes = 0u64;
    let mut total_secs = 0.0f64;
    let mut total_tt_hits = 0u64;
    let mut total_tt_probes = 0u64;
    let mut total_eval = 0u64;

    for case in &suite {
        let mut engine = ClassicEngine::new(ClassicEngineConfig {
            tt_megabytes: tt_mb,
            ..ClassicEngineConfig::default()
        });
        let result = engine.search(&case.game, SearchLimits::depth(depth));
        let secs = result.elapsed.as_secs_f64();
        total_nodes += result.stats.nodes;
        total_qnodes += result.stats.qnodes;
        total_secs += secs;
        total_tt_hits += result.stats.tt_hits;
        total_tt_probes += result.stats.tt_probes;
        total_eval += result.stats.eval_calls;

        println!(
            "{:<26} {:<11} score={:>6} d={:>2} nodes={:>10} q={:>9} time={:>7.3}s nps={:>10.0} tt={:>7}/{:<7} pv={}{}",
            case.name,
            case.category,
            result.score,
            result.depth,
            result.stats.nodes,
            result.stats.qnodes,
            secs,
            result.nodes_per_second(),
            result.stats.tt_hits,
            result.stats.tt_probes,
            result.principal_variation.len(),
            if result.stats.pv_truncations == 0 { "" } else { " !PV" },
        );
    }

    println!();
    println!("Итого позиций : {}", suite.len());
    println!("Итого узлов   : {total_nodes}");
    println!("  qsearch     : {total_qnodes}");
    println!("Оценок        : {total_eval}");
    println!("TT hit/probe  : {total_tt_hits}/{total_tt_probes}");
    println!("Время         : {:.3} с", total_secs);
    println!(
        "Средний NPS   : {:.0}",
        if total_secs > 0.0 {
            total_nodes as f64 / total_secs
        } else {
            0.0
        }
    );
}
