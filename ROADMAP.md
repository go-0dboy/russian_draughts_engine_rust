# План — сила без баз окончаний

Цель: конкурентоспособность с сильными профессионалами на фиксированных
железе и контроле времени. Профессиональный уровень не подтверждён.

| Состояние | Этап |
|---|---|
| Готово, проверяется release gates | Правила, movegen, классический поиск, TT, API/протокол, stop |
| Готово | Corpus V2 STATIC, RDTUNE3, веса, audit, прогресс |
| Реализовано, сила экспериментальная | HCE Adam, integer polish, diagnostics, QRESOLVED, matched STATIC |
| Готов базовый инструмент | Reference stability/agreement, парные GSPRT, SPSA |
| Сейчас | Контролируемый STATIC/QRESOLVED матч, затем сравнение с production и equal-time |
| Далее | Pattern evaluation и проверка стоимости evaluator |
| Далее | Search telemetry, более независимый reference с альтернативами, обоснованные изменения поиска |
| Далее | Strength Lab, self-play generations, независимые confirmation arenas |
| Далее | Турнирные часы, затем SMP при измеренной пользе |
| Финал | Независимые данные и внешние матчи, фиксированный релиз v2.0 |

Production-параметры сохраняют принятую основу; кандидаты не принимаются по loss.
Версии tooling не означают рост Elo. [Подробная методика](docs/PLAN_RU.md).
