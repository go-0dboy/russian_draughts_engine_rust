# Инструменты: команды и восстановление

Все команды выполняются из корня проекта, Python 3.10+. Долгие процессы
показывают этап/счётчик или heartbeat. Ошибки краткие; `RD_DEBUG=1` включает
трассировку. Не путайте число workers (независимые пары) с SMP одного поиска.

## Корпус
```bash
python3 tools/corpus_v2.py build --pdn ~/storage/downloads/russian2012.pdn --out data/corpus_v2 --seed 20260930 --min-ply 6 --max-per-game 96
python3 tools/corpus_v2.py audit data/corpus_v2
python3 tools/qresolved.py --corpus data/corpus_v2 --eval config/eval_default.txt --out data/corpus_qresolved_v1 --node-limit 10000
python3 tools/matched_static.py --static data/corpus_v2 --qresolved data/corpus_qresolved_v1 --out data/corpus_static_matched_q1
```
Существующие завершённые корпуса не перезаписываются. Сборка атомарно публикует
результат; resume сборки пока отсутствует. Audit читает закрытые роли только
для технической проверки целостности/пересечений, не для настройки модели.

## Два сопоставимых обучения
```bash
python3 tools/hce_v2.py --corpus data/corpus_static_matched_q1 --base config/eval_default.txt --out data/eval/static_01 --epochs 30 --batch 4096 --seed 20260930 --l2 0.0000002 --scale 626.751624 --polish-passes 0
python3 tools/hce_v2.py --corpus data/corpus_qresolved_v1 --base config/eval_default.txt --out data/eval/qresolved_01 --epochs 30 --batch 4096 --seed 20260930 --l2 0.0000002 --scale 626.751624 --polish-passes 0
```
Это настройки текущего эксперимента, не универсальный optimum. Default L2
сохранён для совместимости; scale по умолчанию auto, fitted на TRAIN.
Сравнение разных представлений даже при одинаковых весах не заменяет матч.
В manifest обучение complete только после успешного выхода tuner; файлы
прерванного запуска не являются принятым кандидатом. Resume обучения отсутствует.

## Reference
```bash
python3 tools/reference_lab.py teacher --corpus data/corpus_v2 --engine target/release/classic_engine --eval config/eval_default.txt --search config/search_default.txt --out data/reference/teacher_01 --limit 200 --budgets 250000 1000000 4000000
python3 tools/reference_lab.py benchmark --corpus data/corpus_v2 --teacher data/reference/teacher_01 --engine target/release/classic_engine --eval data/eval/qresolved_01/candidate.txt --search config/search_default.txt --out data/reference/qresolved_01 --budgets 10000 50000 100000
```
Teacher привязан к исходному manifest. Для старого teacher используйте тот же
STATIC corpus, а не matched/QRESOLVED каталог. Завершённые запуски неизменяемы;
повтор той же команды возобновляет незавершённый только при полной identity.

## Матч
```bash
python3 tools/match.py --engine-a target/release/classic_engine --engine-b target/release/classic_engine --engine-a-eval data/eval/qresolved_01/candidate.txt --engine-b-eval data/eval/static_01/candidate.txt --engine-a-search config/search_default.txt --engine-b-search config/search_default.txt --openings data/corpus_v2/arena.rdopen --pairs 500 --nodes 100000 --workers 2 --tt 16 --seed 20261002 --elo0 0 --elo1 10 --adjudication abort --state data/matches/q_vs_static_01.json
```
A = QRESOLVED, B = STATIC. В 1.11.1 допустим также examples/classic_server.
2 workers = 4 процесса движка. TT задаётся на каждый процесс; память также нужна
Python, доскам и служебным структурам. Повтор команды возобновляет checkpoint.
Не менять binary/eval/search/openings/tool во время опыта. Для сравнения по
времени замените --nodes на --time-ms и используйте НОВЫЙ state; workers=1
предпочтительнее для уменьшения конкуренции за CPU. Контроль времени выберите
и зафиксируйте до запуска; телефон нагревается и меняет частоту.

## Старые инструменты

compile_corpus.py / classic_corpus (RDTUNE2) сохранены для воспроизведения
ранних опытов. Для новых корпусов используйте corpus_v2.py.
classic_server сохранён для старых baseline и фиксированного packed протокола.
Подробности SPSA: `python3 tools/tune_search.py --help`, config/search_spsa.json.
