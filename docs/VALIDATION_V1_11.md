# Проверка v1.11.0

Rust 1.98.1 установлен в среде подготовки. Полный tools/verify_release.py прошёл:
source hygiene, Python syntax, Python tests, cargo check --release --all-targets,
clippy --release --all-targets -D warnings, cargo test --release, perft(10),
product protocol, реальный STATIC -> QRESOLVED -> HCE с polish и CSV.

Perft(10): 22449922. Fixed-node: move 0000016f00000000, score 2, depth 12,
nodes 100000 — совпал с зафиксированным эталонным tuple. Бинарника v1.7.0
в этой среде нет; прямого A/B с ним здесь не проводилось.

Дополнительно прошли:
- 17 тестов Corpus V2, включая побайтовую эквивалентность прогресса;
- 3 теста QRESOLVED transport/provenance/protected-role filtering;
- 6 тестов match harness;
- Rust-тесты mandatory capture, promotion capture, terminal leaf exclusion,
  root restoration after limit, quiet exact score;
- тесты polish normalization, full TRAIN improvement and DEV veto.

Не изменены файлы production search/eval/TT, rules, public API, engine protocol
и config/eval_default.txt/config/search_default.txt относительно v1.10.1.

Полный пользовательский корпус недоступен в среде сборки. Производительность
QRESOLVED на нём и игровое усиление не измерены. Реальные длительные эксперименты
должны начинаться с пилота и заканчиваться парными матчами, не одним loss.
