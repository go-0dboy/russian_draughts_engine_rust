# Проверки и выпуск

Основная команда из корня проекта:
```bash
python3 tools/verify_release.py
```
Она завершается ошибкой при первом провале. Отсутствующий compiler или бинарник
не считается успешной проверкой. Во время длительных команд выводится heartbeat.

Обязательные проверки: SHA256/hygiene, Python syntax, contract-тесты корпуса и
QRESOLVED, Rust fmt/check/clippy (-D warnings), все Rust tests, perft(10),
production protocol lifecycle, настоящий exporter→training→QRESOLVED pipeline,
matched STATIC и scale, обе версии протокола и смешанные матчи, упаковка/обновление.

perft(10) = 22449922. Эталон initial search при 100000 nodes / TT16:
`move 0000016f00000000 score 2 depth 12 nodes 100000`.
Время не сравнивается. Если data/baselines/v1.7.0/classic_server отсутствует,
проверяется сохранённый tuple, а не заявляется живое сравнение с v1.7.0.
Один tuple не доказывает полную эквивалентность всех позиций.

CI выполняет тот же verifier и отдельно MSRV Rust 1.70.0. Rust formatting
обязателен начиная с 1.11.1. Python отформатирован Black 24.10.0; для обычного
запуска инструментов Black не нужен.

## Упаковка
```bash
python3 tools/check_source_hygiene.py --write-checksums
python3 tools/verify_release.py
python3 tools/build_release.py dist
```
Checksums обновляют после намеренных изменений исходников, не для сокрытия
повреждения неизвестного архива. Release-тест строит два побайтно одинаковых ZIP,
проверяет dry-run, upgrade с сохранением data/target/.git/пользовательского файла,
rollback, отказ на изменённый файл, лишний файл, traversal и symlink.

## Статистика и сила

Матчевый scheduler записывает пары по исходному id, а не по скорости завершения.
Каждый worker владеет двумя отдельными процессами и их TT. Ошибка протокола
прерывает эксперимент; она не должна превращаться в поражение кандидата.
`abort` исключает целую пару из pentanomial статистики.

Заранее задаются hypotheses, alpha/beta, seed, openings, максимальное число пар.
`continue` при достижении лимита означает неопределённый результат. `accepted`
означает решение данного статистического теста, а не автоматическую замену
production. После fixed-node нужен equal-time тест и независимое подтверждение.
Время на ход (`--time-ms`) ещё не полноценные турнирные часы с increment.

Reference agreement со своим teacher — диагностическая метрика. Stability
включает неизменность хода и score tolerance; отдельно выводится move_stable.
Нет точного oracle, MultiPV/top-2/regret и доказанных лучших ходов.
FINAL текущего корпуса retrospective, не новый blind holdout.
