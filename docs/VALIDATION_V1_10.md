# v1.10.1 — исправление проверки корпуса

По предоставленному пользователем логу v1.10.0 в Termux прошли cargo check,
clippy с -D warnings, cargo test, perft(10)=22449922 и product protocol.
Полная verify_release остановилась на ошибке тестовой PDN-выборки.

Исправлены только Python-тесты, версия пакета и документация. Все файлы Rust
в src/ побайтово совпадают с v1.10.0. Протокольные номера клеток переводятся
в PDN; третьим полуходом обеспечивается экспорт позиции после двух полуходов.
До split проверяются число партий, число samples, отсутствие ошибок парсера.

Локально прошли 14 тестов Corpus V2 и 6 тестов hardening. Исправленный тест
с реальным Rust-бинарником здесь не запускался: Cargo отсутствует.
Для быстрой проверки можно использовать уже собранные бинарники v1.10.0:

```bash
python3 tools/test_corpus_engine.py
```

После успешного результата нужен полный `python3 tools/verify_release.py`.
Повышение силы игры и обучение на реальном корпусе этой проверкой не доказаны.

---

# v1.10.0 — проверка и ограничения

Дата: 2026-09-30. Статус: **source candidate / Rust gates pending**.

В среде подготовки отсутствуют cargo/rustc. Поэтому этот архив нельзя честно
называть скомпилированным и полностью проверенным релизом. Он содержит исходники
реализации и обязательные проверки для машины с Rust. Не устанавливайте поверх
рабочего эталона до успешного `python3 tools/verify_release.py`.

Проверено исполнением:

- 12 Python-тестов Corpus V2/reference: воспроизводимость, независимость data
  order от source order, weights/leakage audit, FINAL guard, corruption/truncation,
  immutable destination, duplicate/conflicting games, strata cap, teacher stability,
  mock production protocol teacher→benchmark;
- 6 существующих Python-тестов hardening/scheduler;
- синтаксис Python и shell updater;
- source hygiene/контрольные суммы перед упаковкой;
- повторная сборка архива с побайтовым сравнением.

Не проверено исполнением здесь:

- компиляция нового Rust exporter и изменённого weighted trainer;
- Rust unit tests, MSRV/clippy, perft, fixed-node baseline comparison;
- actual-engine `test_corpus_engine.py`;
- реальный полный корпус, объём 3–5 млн, время и память на телефоне;
- качество обученных коэффициентов, GSPRT и профессиональный уровень.

Python synthetic/mock tests не являются тестами правил русских шашек.
Новый Rust unit test проверяет инвариант weighted loss при разбиении sample;
existing exact-serialized-score tests сохраняются. Реальная интеграционная
проверка строит PDN с ходами самого движка, компилирует V2 и запускает одну эпоху.
Все эти Rust gates включены в verify_release/CI и должны быть выполнены.

По байтам сохранены production search, TT, eval, movegen/game/API/protocol,
search/eval config относительно приложенного v1.9.4. Изменён офлайн HCE tuner,
добавлен офлайн exporter и Python-инструменты. Само сохранение исходников ядра
не подменяет повторной сборки и теста его бинарника.
