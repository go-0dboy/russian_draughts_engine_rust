# Architecture — Russian Draughts Classic Engine v1.9.4

## 1. Baseline and scope

The accepted strength baseline is **v1.7.0**. v1.9.4 is built from that search
and HCE line and adds product boundaries, lifecycle safety, corpus isolation and
release engineering. The rejected v1.7.1 workspace-reuse and v1.8.0 history
experiments are not present; see `EXPERIMENTS.md`.

## 2. Layering

```text
untrusted app / file / protocol input
        ↓ validation
checked API (`src/api.rs`) / `rd-classic/1`
        ↓
Game + exact rules + canonical move generation
        ↓
ClassicEngine
 ├─ EvalParams / SearchParams
 ├─ TranspositionTable
 ├─ history / killers / countermoves
 └─ iterative-deepening PVS / qsearch / LMR / extensions
        ↓
SearchResult
```

Validation belongs at external boundaries. Recursive search keeps the trusted,
allocation-conscious core unchanged.

## 3. Position and move contracts

`Position` stores white occupancy, black occupancy, king mask and side to move
on the 32 playable squares. Checked construction rejects overlap, kings on empty
squares and more than 12 pieces per side.

Canonical move generation implements mandatory captures, backward man captures,
flying kings, promotion during a capture and captured-piece blocking until the
complete sequence ends.

The hot-path complete move is exactly one internal `u64` and includes captured
mask plus cached metadata. Product code uses stable
`MoveSpec { from, to, captures }`; resolving a `MoveSpec` always matches it
against canonical legal moves. The checked product `legal_moves()` first counts
moves exactly and then allocates exactly-sized storage, so external code does
not rely on the core's historical 256-entry caller buffer.

The fixed 256-entry buffer remains a validated internal search/movegen
convention. A deterministic 100,000-position stress regression checks the exact
counter against generation and asserts the capacity, but this is evidence rather
than a mathematical bound. Before v2.0 the convention must either receive a
formal/exhaustive legal-move bound proof or be replaced at the core boundary by
an overflow-detecting form.

## 4. Game state and terminal semantics

Board placement alone is not sufficient search identity. `Game` also owns exact
repetition history/fingerprint, rule counters and incremental Zobrist state.
Threefold repetition is claimable; unsupported official endings surface as
`NeedsAdjudication` instead of being guessed.

`CheckedGame::play` and legacy packed play reject mutation after
`GameStatus::Finished`, reject `NeedsAdjudication`, reject exhausted history and
resolve moves against the legal set. Direct `Game` methods remain the trusted
internal API.

Search checks remaining history capacity before any recursive temporary move.
If the fixed history cannot safely extend, search returns the best completed or
fallback move with stop reason `history_limit` rather than panicking.

## 5. Search cancellation lifecycle

Cancellation is **search-scoped**, not a persistent sequence/flag on the
engine. `ClassicEngine::control()` prepares a fresh `SearchControl` for exactly
the next `search()`; `search()` consumes it. Without a prepared handle it creates
an unexposed fresh token.

Consequences:

- `go` can prepare the token before spawning the worker, so an immediate `stop`
  cannot be lost;
- a repeated or late stop on a finished search changes only that old token;
- the following search always receives a fresh token;
- terminal searches follow the same isolation rule.

The production protocol keeps only the active search token and clears it when
the corresponding search result event is committed.

## 6. Classical search invariants

- iterative deepening PVS/alpha-beta;
- mandatory captures are searched rather than stand-patted in qsearch;
- TT bounds require compatible extension budget; incompatible entries may be
  move hints only;
- PV is validated before return;
- root `Game` is not mutated;
- recursive state uses make/undo rather than cloning the 513-entry history;
- the accepted v1.7.0 HCE/search parameters remain production defaults;
- rejected staged-MovePicker/workspace/history experiments are absent.

`config/search_default.txt` is the single editable production search source and
is embedded for `SearchParams::default()`.

## 7. Product protocol

`src/bin/classic_engine.rs` is a line-oriented adapter whose input thread stays
alive during search. The worker owns the `ClassicEngine` while searching and
returns it through an event channel. The command thread can therefore process
`ping`, `stop` and `quit` while a search is active.

`rd-classic/1` is intentionally strict: malformed optional values, zero depth,
duplicate capture squares and unexpected trailing arguments are errors rather
than silently normalized commands. See `PROTOCOL.md`.

`position` reconstructs a fresh game from board placement and side only. Exact
live-game resume must use `new` followed by legal `play` commands until a future
versioned full-state snapshot contract is introduced.

## 8. Evaluation and corpus

The production evaluator is tapered MG/EG HCE with versioned text parameters
and a score-only recursive hot path. Historical game WDL is noisy supervision,
not exact truth.

Corpus roles are selected by a semantic fingerprint of start position and move
sequence: 80% train, 10% validation, 5% test, 5% arena. Arena games never enter
HCE tuning. Exact positions duplicated across HCE splits are retained only in
`test > validation > train`. Arena openings are filtered if any position reached
by replay overlaps any retained HCE position.

Normal tuning optimizes train and selects on validation. Frozen test evaluation
is a separate report-only mode. `tools/compile_corpus.py` records source,
compiler and output SHA-256 values plus seed/parameters for reproducibility.

## 9. Match/SPSA laboratory

Color-reversed opening pairs feed pentanomial GSPRT. Protocol reads and process
shutdown are bounded, stderr is drained through a file, and failed startup is
reaped.

Workers receive new pairs dynamically, but statistical state is committed in
ascending pair id. Once SPRT has a decision, speculative running workers are
stopped/drained and their extra results do not alter the accepted ordered
sample. SPSA uses the same scheduling discipline.

## 10. Release engineering

The quality toolchain is pinned in `rust-toolchain.toml` to Rust 1.98.1; Cargo
MSRV remains 1.70. CI runs source hygiene, Python tests, Rust check, Clippy with
`-D warnings`, Rust tests, canonical perft and the real product-protocol smoke.

`rustfmt` remains non-blocking for v1.9.4 because making it blocking requires a
one-time repository-wide formatting-only normalization of historical sources.
That change must not be mixed with search/product behavior.

Source releases are manifest-only and byte-reproducible. `SOURCE_SHA256SUMS.txt`
owns every shipped file except itself; updater validation rejects path traversal,
symlinks, checksum mismatches and unowned payload before modifying the project.

## 11. Product direction

The Rust core remains independent from host applications. Future integration
layers (C ABI, JNI/Android, optional WASM) must expose checked/versioned DTOs,
not internal packed moves or private rule/search structs. Tablebases and opening
books are separate data subsystems; tablebase probing is required before v2.0.
