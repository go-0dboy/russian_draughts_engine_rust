use russian_draughts_classic::{
    benchmark_suite, ClassicEngine, ClassicEngineConfig, SearchLimits, ENGINE_VERSION,
};

fn main() {
    let depth = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(10);
    let sizes: Vec<usize> = std::env::args()
        .skip(2)
        .filter_map(|s| s.parse::<usize>().ok())
        .collect();
    let sizes = if sizes.is_empty() {
        vec![8, 16, 32, 64, 128, 256]
    } else {
        sizes
    };

    let suite = benchmark_suite();
    println!("Сравнение размеров TT на корпусе v{ENGINE_VERSION}");
    println!("Глубина: {depth}");
    println!("Позиций: {}", suite.len());
    println!();
    println!(
        "{:>7} {:>14} {:>11} {:>12} {:>12} {:>9}",
        "TT МБ", "узлов", "время,с", "NPS", "hit/probe", "hash‰"
    );

    for tt_mb in sizes {
        let mut total_nodes = 0u64;
        let mut total_secs = 0.0f64;
        let mut hits = 0u64;
        let mut probes = 0u64;
        let mut hash = 0u64;
        let mut count = 0u64;
        for case in &suite {
            let mut engine = ClassicEngine::new(ClassicEngineConfig {
                tt_megabytes: tt_mb,
                ..ClassicEngineConfig::default()
            });
            let result = engine.search(&case.game, SearchLimits::depth(depth));
            total_nodes += result.stats.nodes;
            total_secs += result.elapsed.as_secs_f64();
            hits += result.stats.tt_hits;
            probes += result.stats.tt_probes;
            hash += result.stats.hashfull_per_mille as u64;
            count += 1;
        }
        let nps = if total_secs > 0.0 {
            total_nodes as f64 / total_secs
        } else {
            0.0
        };
        println!(
            "{:>7} {:>14} {:>11.3} {:>12.0} {:>5}/{:<6} {:>9}",
            tt_mb,
            total_nodes,
            total_secs,
            nps,
            hits,
            probes,
            hash.checked_div(count).unwrap_or(0),
        );
    }
}
