# v1.11.1 — release/tooling hardening

- Match/SPSA support production, legacy and mixed protocols; real-process tests.
- Reference preflight before output creation; heartbeat and separate move stability.
- Integrated matched STATIC and fixed scale, concise CLI errors, progress during gates/SPSA.
- Updater validates ZIP symlinks before extraction and completes backup before mutations.
- Manifest required files and sorted-record audit; Cargo.lock included in release.
- Mandatory rustfmt, unified CI/local gates, reproducible archive and update regression tests.
- Documentation reconciled with implementation; no strength promotion.

# v1.11.0

- Capture-only QRESOLVED с восстановлением истории, ограничениями и provenance.
- Integer polish с предложениями на TRAIN subset, проверкой всего TRAIN и выбором DEV.
- CSV по классам и калибровке; прогресс HCE/Q-процессов.
- Оптимизация Python Corpus V2 и корректный PDN-тест включены в полный релиз.
- Производственный поиск и веса не изменены; сила пока не измерена.

# v1.10.1 — исправление интеграционного теста

Исправлено преобразование клеток протокола в PDN. Тестовые партии продлены до трёх полуходов; выборка берётся после второго полухода. Ошибки парсера и неполный экспорт теперь диагностируются до разделения выборки. Production Rust-код не изменён.

# v1.10.0 — offline Corpus V2 candidate

- Adds a Rust PDN/feature stream bridge using the existing rules and evaluator.
- Adds disk-backed semantic game splitting and stratified STATIC sampling.
- Introduces RDTUNE3 with per-game post-filter weights and SHA256 manifests.
- Removes exact exported-position overlap across all five roles; emits legacy
  arena openings only from retained arena positions.
- Adds weighted RDTUNE3 train/dev support to the existing HCE Adam tuner;
  old RDTUNE2 records retain unit weights.
- Adds reference teacher/benchmark with replayed history, immutable provenance,
  budget stability/agreement and resumable per-position results.
- No search/production eval changes. No QRESOLVED, pattern or auto-promotion claims.
- Rust compilation and actual-engine tests are mandatory host gates, not yet
  executed in the packaging environment.

# Changelog

## 1.9.4 — acceptance cleanup

Supersedes the unaccepted v1.9.3 candidate. The product/search semantics remain
those of the v1.9.2 consolidation; this release only closes the remaining
mandatory quality-gate finding exposed by the pinned Rust 1.98.1 Clippy:

- replaces the guarded integer division in `classic_tt_sweep` with
  `checked_div(...).unwrap_or(0)`, removing `manual_checked_ops` without changing
  the diagnostic result;
- keeps the accepted v1.7.0 search/HCE production configuration unchanged.

No search/HCE strength heuristic is changed in v1.9.4.

## 1.9.3 — acceptance candidate, not promoted

Superseded the unaccepted v1.9.2 candidate and fixed its earlier Clippy findings,
but the mandatory `cargo clippy --release --all-targets -- -D warnings` gate on
the pinned Rust 1.98.1 toolchain then exposed `manual_checked_ops` in the
`classic_tt_sweep` diagnostic example. It was never promoted to the production
baseline. v1.9.4 supersedes it.

## 1.9.2 — product-foundation candidate, not promoted

Built from the accepted v1.7.0 search/HCE baseline. Rejected v1.7.1 workspace
reuse and v1.8.0 history changes are not included.

Product/lifecycle changes:

- adds checked product Position/Move/Game API and stable `MoveSpec`;
- product legal-move storage is exact-sized rather than exposing the core's
  fixed 256-entry buffer convention;
- adds non-panicking `ClassicEngine::try_new` for untrusted configuration;
- replaces persistent stop sequencing with a **search-scoped cancellation
  token**, preventing a late/repeated stop from cancelling the next search;
- search stops safely with `history_limit` if recursive temporary history cannot
  be extended;
- `CheckedGame` rejects play after terminal status, during required external
  adjudication and at exhausted history capacity;
- adds asynchronous `rd-classic/1` adapter with active-search stop/ping/quit;
- makes the production protocol strict: malformed optional depths, zero values,
  duplicate captures and trailing arguments are rejected;
- keeps packed `Move(u64)` internal/laboratory rather than a product wire format.

Laboratory/data changes:

- changes match/SPSA execution from fixed batches to dynamic worker scheduling
  while committing statistical state strictly by pair id;
- stops/drains speculative workers after an ordered early SPRT decision;
- partitions corpus by semantic game fingerprint: 80% train / 10% validation /
  5% test / 5% arena;
- arena games never generate HCE samples;
- removes exact-position leakage across HCE splits with priority
  `test > validation > train`;
- filters arena opening lines whose replay intersects retained HCE positions;
- adds reproducible corpus provenance hashes/seed/parameters;
- normal HCE tuning never reveals test loss; `--evaluate-test` is a separate
  report-only frozen-candidate mode.

Release/quality changes:

- adds `rust-toolchain.toml` pinned to Rust 1.98.1 quality components while
  retaining Cargo MSRV 1.70;
- Clippy `-D warnings` is a release gate; historical rustfmt normalization is
  intentionally kept as a later formatting-only change;
- adds manifest-only byte-reproducible source packaging and excludes generated
  Python/cache junk;
- emits a standalone bootstrap updater for safe upgrades from pre-v1.9;
- updater validates owned checksums, rejects unsafe/symlink/unowned payloads
  before installation and preserves user/runtime paths;
- moves historical review into `docs/audits/` and experiment evidence into
  `EXPERIMENTS.md`.

No accepted search/HCE strength heuristic was changed in v1.9.2.

## 1.9.1 — acceptance candidate, not promoted

The first consolidated archive after the v1.9.0 RC failed the mandatory
`cargo check --release --all-targets` gate because a newly added regression
test referenced `SearchParams` without importing it. It was never promoted or
used as a production baseline. v1.9.2 contained the corrected test but later failed the mandatory Clippy gate;
v1.9.3 supersedes it.

## 1.9.0 — release candidate, not promoted

The first product-foundation candidate exposed remaining Clippy and lifecycle/API edge cases during acceptance. It was never promoted to the production baseline. Those findings were folded into the product-foundation line; there is no separate accepted v1.9.0 line.

## 1.7.0 — accepted reliability baseline

- bounded match protocol I/O and process shutdown;
- sequence-based external-stop lifecycle fixed the earlier lost-at-start race;
- terminal precedence regression for immobility versus draw/adjudication;
- accepted search/HCE line remains the v1.5.1 baseline.

v1.9.4 supersedes the sequence-based stop lifecycle with search-scoped tokens.
Earlier/non-promoted experiment history is in `EXPERIMENTS.md`.
