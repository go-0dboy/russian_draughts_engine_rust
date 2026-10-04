use russian_draughts_classic::{perft, Position};
use std::time::Instant;

fn main() {
    let depth = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(8);

    let pos = Position::initial();
    let start = Instant::now();
    let nodes = perft(pos, depth);
    let elapsed = start.elapsed();
    let seconds = elapsed.as_secs_f64();
    let nps = if seconds > 0.0 {
        nodes as f64 / seconds
    } else {
        0.0
    };
    println!(
        "Канонический perft русских шашек v{}",
        russian_draughts_classic::ENGINE_VERSION
    );
    println!("Глубина       : {depth}");
    println!("Узлов         : {nodes}");
    println!("Время         : {:.6} с", seconds);
    println!("Узлов/с       : {:.0}", nps);
}
