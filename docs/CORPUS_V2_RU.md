# Corpus V2 STATIC (актуально для v1.11.1)

Статус: кандидат офлайн-инструментов на базе v1.9.4. Исходный PDN и обученные
веса в ZIP не входят. Production eval/search не заменяются. Полный план:
[PLAN_RU.md](PLAN_RU.md). Ограничения проверки: [VALIDATION_V1_10.md](VALIDATION_V1_10.md).

## 1. Можно выполнять сейчас: проверка в отдельной папке

Сначала распакуйте новый архив отдельно от рабочей установки и перейдите в
папку проекта. Команды ниже запускаются из её корня. Новый код Rust ещё должен
пройти эти проверки на вашей машине. До успеха НЕ заменяйте production-установку.

```bash
python3 tools/verify_release.py
```

Скрипт выполняет Python/Rust проверки, perft, production protocol, сборку нового
экспортёра и тюнера, реальную интеграцию PDN → RDTUNE3 → один шаг HCE.
Если эталон v1.7.0 доступен в ожидаемой старым gate папке, используется его
сравнение. Вывод отдельных проверок не следует подменять полной приёмкой.

## 2. После успешных gates: Corpus V2

Путь к PDN ниже — пример. Замените его своим реальным файлом. Выходная папка
должна быть новой: существующие результаты инструмент не перезаписывает.

```bash
python3 tools/corpus_v2.py build \
  --pdn data/source/russian2012.pdn \
  --exporter target/release/classic_corpus_stream \
  --out data/corpus/v2-static-001 \
  --seed 20260930 \
  --min-ply 6 \
  --max-per-game 96

python3 tools/corpus_v2.py audit data/corpus/v2-static-001
```

Временная SQLite создаётся на том же диске, что и результат. Для миллионов
samples обеспечьте несколько ГБ свободного места; точный максимум зависит от
длины партий/индексов. Полная сборка не имеет resume: при сбое перезапустите её.
Частично сформированный корпус не публикуется как готовая папка.

Результат: пять RDTUNE3-файлов, пять replay-файлов, arena.rdopen, rejected.json,
manifest.json. Количество samples честно определяется корпусом и фильтрами.
Сборка не гарантирует 3–5 млн и не дублирует samples для достижения этой цифры.
Отчёт показывает материал, фазу, тихие/тактические позиции и превращения.

`arena.rdopen` включает до 4096 уникальных экспортированных позиций на 6..14
полуходах из ARENA, только с обычной начальной расстановкой. После строгого
фильтра пересечений их может быть мало или ноль — тогда не начинайте матчи
на пустом/слишком узком наборе. Нужен отчёт и заранее выбранный новый источник,
а не поиск seed, дающего кандидату удобную arena.

## 3. Weighted STATIC HCE pilot

RDTUNE3 напрямую читается обновлённым classic_tune_eval. Предпочтительно запускать
обёртку, которая сначала проверяет контрольные суммы и сохраняет provenance.
Все веса партий учитываются в TRAIN gradient, train-only logistic calibration,
float/integer loss и DEV selection. Финальные параметры по-прежнему целочисленные
и нормализуются существующим кодом. Integer polish — следующий этап.

```bash
python3 tools/hce_v2.py \
  --corpus data/corpus/v2-static-001 \
  --base config/eval_default.txt \
  --out data/lab/hce-v2-pilot-001 \
  --epochs 3 --batch 4096 --seed 20260930
```

Для полноценного запуска задайте новый `--out` и заранее выбранное число эпох.
В папке: candidate.txt, training.log, manifest.json. Это кандидат, не champion.
Не копируйте его в config/eval_default.txt без игровых gates. Тюнер использует
память под все train/dev samples; out-of-core обучение ещё не реализовано.
REFERENCE/ARENA/FINAL не участвуют в обучении. `--evaluate-test` для V2 запрещён.

## 4. Замороженный teacher: начните с 200 позиций

Teacher — принятые eval/search и бинарник, не только что обученный кандидат.
Рекомендуется копия бинарника: его хеш должен оставаться неизменным.
Пути ниже — пример расположения замороженных файлов.

```bash
python3 tools/reference_lab.py teacher \
  --corpus data/corpus/v2-static-001 \
  --engine target/release/classic_engine \
  --eval config/eval_default.txt --search config/search_default.txt \
  --out data/lab/reference-001 \
  --limit 200 --per-game 4 \
  --budgets 250000 1000000 4000000 \
  --tt 16 --timeout 600
```

На каждом бюджете восстанавливаются root и все ходы до sample; TT очищается.
Результаты сохраняются после каждой позиции. Тот же запуск возобновит прерванный
при точном совпадении hashes/параметров. Завершённый запуск неизменяем.
Один sample с прерыванием между бюджетами будет пересчитан целиком.

`stable` означает: один ход на всех бюджетах, приемлемая причина остановки,
ненулевая глубина и разброс score не более `--score-tolerance` (по умолчанию 20).
Это эвристическая устойчивость, не доказательство лучшего хода. Все нестабильные
позиции тоже сохраняются. acceptable_moves=null; top-2/regret не выдумываются.

## 5. Проверка кандидата

```bash
python3 tools/reference_lab.py benchmark \
  --corpus data/corpus/v2-static-001 \
  --teacher data/lab/reference-001 \
  --engine target/release/classic_engine \
  --eval data/lab/hce-v2-pilot-001/candidate.txt \
  --search config/search_default.txt \
  --out data/lab/reference-candidate-001 \
  --budgets 10000 25000 50000 100000 250000 \
  --tt 16 --timeout 600
```

Отчёт: agreement с устойчивым teacher по бюджету, число eligible/usable, все
сырые ответы. Для терминальных/claim/adjudication ответов не изобретается ход:
они отмечаются unusable для этой конкретной метрики. Это не поражение в партии.

Затем используйте существующий match.py с `--openings .../arena.rdopen`, одинаковыми
eval/search условиями, сменой цветов и заранее определёнными статистическими
порогами. Ни reference agreement, ни loss автоматически не продвигают кандидата.

## Пока НЕ запускать как готовую возможность

QRESOLVED и integer polishing реализованы: см. HCE_V3_RU.md и TOOLS_RU.md.
Pattern training, автоматический generation loop и promotion пока не реализованы. Честный blind FINAL нельзя получить
перетасовкой прежнего корпуса. Не объявляйте профессиональную силу без внешних
контрольных матчей на фиксированных условиях.
