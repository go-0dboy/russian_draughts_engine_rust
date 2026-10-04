# Russian Draughts Classic — v1.11.1

Классический движок **русских шашек 8×8**, без нейросети и баз окончаний.
Bitboards, обязательные многоударные взятия, летающие дамки, превращение во
время взятия, PVS/alpha-beta, TT и обучаемая handcrafted evaluation (HCE).

Цель — подтверждённая игра на уровне сильных профессионалов при фиксированных
железе и контроле времени. **Этот уровень пока не установлен.** Версия 1.11.1
исправляет инструменты и выпуск, не вводит новую принятую оценку или поиск.

## Сборка и проверка

Linux / Termux; Rust 1.98.1 закреплён в rust-toolchain.toml. Python 3.10+,
стандартная библиотека; для обновлятора нужны Bash, unzip, GNU coreutils.
На macOS для update.sh установите GNU coreutils/findutils и используйте их
в PATH; автоматическая проверка этого релиза выполнена на Linux, не на macOS.
Rust API также проверяется на минимальном Rust 1.70.0.

```bash
cargo build --release --bins --examples
python3 tools/verify_release.py
```

На Termux при отсутствии инструментов: `pkg install rust python clang make unzip coreutils`.
Если Termux использует пакетный Rust без rustup, используйте поддерживаемый
дистрибутивом compiler (не ниже MSRV); это отличается от закреплённой CI-среды.

## Анализ и протокол

```bash
cargo run --release --example classic_analyse -- 12 16
./target/release/classic_engine --eval config/eval_default.txt --search config/search_default.txt --tt 16
```

В интерактивном протоколе: `go nodes 100000 64`, затем дождаться `bestmove`;
`quit` завершает процесс. Не отправляйте `quit` сразу после `go` в pipe:
production-адаптер асинхронный и остановит поиск.

## Инструменты

| Инструмент | Назначение |
|---|---|
| tools/corpus_v2.py build / audit | STATIC RDTUNE3, веса, дедупликация и аудит |
| tools/qresolved.py | capture-only QRESOLVED TRAIN/DEV |
| tools/matched_static.py | STATIC с теми же наблюдениями и весами, что QRESOLVED |
| tools/hce_v2.py | обучение HCE, фиксированный scale, диагностика и polish |
| tools/reference_lab.py teacher / benchmark | устойчивость и совпадения с teacher |
| tools/match.py | парные матчи и pentanomial GSPRT |
| tools/tune_search.py | SPSA числовых параметров поиска |
| tools/verify_release.py | полный обязательный локальный gate |
| tools/build_release.py | воспроизводимый архив исходников |

**Рекомендуемый бинарник для всех новых экспериментов — classic_engine.**
Матчи/SPSA автоматически поддерживают также старый examples/classic_server
и смешанные пары. Reference требует classic_engine. Внутренний packed u64
и публичный MoveSpec — разные форматы; клиент переводит их автоматически.

- [Пошаговая работа с инструментами](docs/TOOLS_RU.md)
- [Методика HCE/QRESOLVED](docs/HCE_V3_RU.md)
- [Формат корпуса](docs/RDTUNE3.md)
- [Тесты и выпуск](TESTING.md)
- [Ревью, ограничения и оптимизация](docs/REVIEW_V1_11_1.md)
- [Архитектура](ARCHITECTURE.md), [API](API.md), [протокол](PROTOCOL.md)
- [План](ROADMAP.md), [история изменений](CHANGELOG.md)

Корпус PDN, обученные кандидаты, персональные данные и бинарники не входят
в репозиторий. Храните эксперименты под data/. Убедитесь, что имеете право
использовать и распространять собственные исходные партии; исходный код — MIT.
Не заменяйте production-веса по одному loss или reference agreement.

Обновление из архива (с резервной копией):
```bash
bash update.sh ~/storage/downloads/russian_draughts_classic_v1.11.1.zip
python3 tools/verify_release.py
```
Завершите старые запуски до обновления: контрольные суммы инструментов и
бинарников входят в identity; старые match/reference checkpoints нельзя
продолжать под новым кодом. Готовые корпуса и кандидаты сохраняются.
