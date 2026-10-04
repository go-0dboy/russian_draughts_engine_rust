# Product API contract — v1.11.1 (semantics retained from v1.9.4)

The recursive engine core is trusted and optimized. Untrusted application data
should enter through `russian_draughts_classic::api`.

## Position

Use `checked_position()` or `CheckedGame::new()` for external positions.
Validation rejects:

- overlapping white/black bitboards;
- king bits outside occupied squares;
- more than 12 pieces per side.

`Position` itself stays a compact public value for engine/internal tooling, so
applications that deserialize arbitrary data must validate it first.

## Moves

`Move(u64)` is the internal hot-path representation and is not a stable wire or
persistent-storage format.

External code uses:

```text
MoveSpec {
  from:     0..31,
  to:       0..31,
  captures: exact 32-bit captured-square mask
}
```

`legal_moves(position)` returns owned exact-sized legal storage and exposes
`MoveSpec` iteration. `resolve_legal_move(position, spec)` accepts a move only
when it resolves to exactly one canonical legal engine move.

## CheckedGame

`CheckedGame` owns `Game` and is the default mutable game boundary for apps.
`play()` and `play_packed_legacy()` reject:

- invalid/illegal/ambiguous moves;
- moves after `GameStatus::Finished`;
- moves while `NeedsAdjudication` is active;
- exhausted fixed game-history capacity.

`ClaimableDraw` remains playable because claiming is optional. If an application
needs hypothetical analysis after an already finished game, it should construct
a separate analysis position/game explicitly rather than mutate the completed
`CheckedGame`.

`legal_moves()` may still be used to inspect board-legal movement; legality of
*continuing the game* is enforced by `play()`.

## Engine construction

External configuration should use `ClassicEngine::try_new(config)`. Invalid
SearchParams, zero TT size and zero time-check interval are returned as
`ClassicEngineBuildError` rather than panicking. `ClassicEngine::new()` remains
a convenience for trusted configuration.

## Search cancellation

Call `engine.control()` once immediately before handing that engine to a search
worker, then retain the returned `SearchControl`. It belongs to exactly that
next search.

```text
let control = engine.control();
spawn(move || engine.search(...));
control.request_stop();
```

Late/repeated requests on the old handle cannot affect a later search. Calling
`control()` again before `search()` intentionally replaces the previously
prepared handle.

## Search near history capacity

Recursive search needs temporary history cells. If no cell remains, it stops
safely with `SearchStopReason::HistoryLimit` and preserves the best completed or
fallback decision instead of allowing `Game::play()` to overflow its fixed
history.

## Stability policy

Stable product contracts are checked Position/Move/Game semantics and the
versioned protocol DTOs. Internal packed bits, search workspace layout, TT entry
layout and diagnostic structs are not promised ABI/storage formats.

A C ABI/JNI/WASM layer will be defined before v2.0 from explicit versioned DTOs,
not by exposing Rust struct memory.
