use std::time::Duration;

use russian_draughts_classic::{
    coordinates, ClassicEngine, ClassicEngineConfig, ClassicSearchDecision, Game, Move,
    SearchLimits, ENGINE_VERSION,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let depth = args.get(1).and_then(|s| s.parse::<u8>().ok()).unwrap_or(12);
    let tt_mb = args
        .get(2)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(64);
    let time_ms = args.get(3).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);

    let config = ClassicEngineConfig {
        tt_megabytes: tt_mb,
        ..ClassicEngineConfig::default()
    };
    let mut engine = ClassicEngine::new(config);
    let game = Game::initial();

    let eval = engine.evaluate(&game);
    println!("Классический движок русских шашек v{ENGINE_VERSION}");
    println!("Позиция              : начальная");
    println!(
        "TT                    : {tt_mb} МБ (фактически {} байт)",
        engine.transposition_table_bytes()
    );
    println!("Статическая оценка    : {}", eval.side_to_move);
    println!("  материал            : {}", eval.material);
    println!("  продвижение         : {}", eval.advancement);
    println!("  PSQT                : {}", eval.psqt);
    println!("  центр               : {}", eval.center);
    println!("  тыловая линия       : {}", eval.back_rank);
    println!("  мобильность         : {}", eval.mobility);
    println!("  мобильность дамок   : {}", eval.king_mobility);
    println!("  структура           : {}", eval.structure);
    println!("  блокированные       : {}", eval.blocked_men);
    println!("  давление промоции   : {}", eval.promotion_pressure);
    println!("  темп                : {}", eval.tempo);
    println!("  изолированные       : {}", eval.isolated_men);
    println!("  поддержанные        : {}", eval.supported_men);
    println!("  слабые на краю      : {}", eval.edge_isolated_men);
    println!("  баланс флангов      : {}", eval.wing_balance);
    println!("  продв. с поддержкой : {}", eval.advanced_supported_men);
    println!("  ходы простых        : {}", eval.man_mobility);
    println!("  ходы в дамки        : {}", eval.promotion_moves);
    println!("  стеснённые дамки    : {}", eval.constrained_kings);
    println!("  фаза MG             : {}/256", eval.phase);

    let limits = if time_ms > 0 {
        SearchLimits {
            max_depth: depth,
            max_nodes: None,
            max_time: Some(Duration::from_millis(time_ms)),
        }
    } else {
        SearchLimits::depth(depth)
    };
    let result = engine.search(&game, limits);

    println!();
    println!("Решение               : {}", decision_text(result.decision));
    println!("Оценка                : {}", result.score);
    println!("Глубина               : {}", result.depth);
    println!("Причина остановки     : {:?}", result.stop_reason);
    println!(
        "PV                    : {}",
        pv_text(&result.principal_variation)
    );
    println!("Узлов                 : {}", result.stats.nodes);
    println!("  quiescence          : {}", result.stats.qnodes);
    println!("Оценок позиции        : {}", result.stats.eval_calls);
    println!(
        "TT hit/probe          : {}/{}",
        result.stats.tt_hits, result.stats.tt_probes
    );
    println!(
        "TT несовм. продлений  : {}",
        result.stats.tt_incompatible_extension_hits
    );
    println!("TT cutoffs            : {}", result.stats.tt_cutoffs);
    println!("TT ранних cutoffs     : {}", result.stats.tt_early_cutoffs);
    println!(
        "Hashfull              : {}‰",
        result.stats.hashfull_per_mille
    );
    println!("Beta cutoffs          : {}", result.stats.beta_cutoffs);
    println!(
        "Cutoff первым ходом   : {}",
        result.stats.first_move_cutoffs
    );
    println!(
        "LMR reductions/retry  : {}/{}",
        result.stats.lmr_reductions, result.stats.lmr_researches
    );
    println!("PVS re-search         : {}", result.stats.pvs_researches);
    println!(
        "Aspiration re-search  : {}",
        result.stats.aspiration_researches
    );
    println!("PV truncations        : {}", result.stats.pv_truncations);
    println!("Максимальный ply      : {}", result.stats.max_ply);
    println!(
        "Время                 : {:.3} с",
        result.elapsed.as_secs_f64()
    );
    println!("Узлов/с               : {:.0}", result.nodes_per_second());
}

fn decision_text(decision: ClassicSearchDecision) -> String {
    match decision {
        ClassicSearchDecision::Move(mv) => format!("ход {}", move_text(mv)),
        ClassicSearchDecision::ClaimDraw => "заявить ничью (троекратное повторение)".to_string(),
        ClassicSearchDecision::ClaimDrawByMove(mv) => {
            format!("заявить ничью ходом {}", move_text(mv))
        }
        ClassicSearchDecision::Terminal(result) => format!("партия завершена: {result:?}"),
        ClassicSearchDecision::NeedsAdjudication(reason) => {
            format!("требуется внешний арбитр/tablebase: {reason:?}")
        }
    }
}

fn pv_text(pv: &[Move]) -> String {
    if pv.is_empty() {
        return "—".to_string();
    }
    pv.iter()
        .copied()
        .map(move_text)
        .collect::<Vec<_>>()
        .join(" ")
}

fn move_text(mv: Move) -> String {
    let sep = if mv.is_capture() { ':' } else { '-' };
    format!("{}{}{}", square_name(mv.from()), sep, square_name(mv.to()))
}

fn square_name(sq: u8) -> String {
    let (file, rank) = coordinates(sq).expect("valid move square");
    let file = (b'a' + file) as char;
    format!("{file}{}", rank + 1)
}
