# `rd-classic/1` production protocol

`classic_engine` is the production line-oriented adapter. On startup:

```text
ready Russian draughts classic engine 1.11.1 protocol rd-classic/1
```

The exact engine name/version come from package metadata; clients should rely on
the `protocol rd-classic 1` contract rather than hard-code the greeting text.

## Parsing rules

The grammar is strict. A supplied malformed optional argument is an error, not a
default. Commands reject unexpected trailing tokens. Depth and node/time budgets
must be positive. Capture lists cannot contain duplicate squares.

Errors are single lines beginning with `error` and do not mutate game/search
state.

## Commands while idle

```text
protocol
version
ping [TOKEN]
new
position WHITE_HEX BLACK_HEX KINGS_HEX white|black
legal
play FROM TO CAPTURES
side
status
go nodes N [DEPTH]
go time MS [DEPTH]
go depth DEPTH
analyze nodes N [DEPTH]
analyze time MS [DEPTH]
analyze depth DEPTH
stop
quit
```

Protocol squares are **1..32**. `CAPTURES` is `-` or a comma-separated list of
unique 1..32 squares. A move is resolved against canonical legal `MoveSpec`
identity (`from`, `to`, exact captured set).

`position` starts a fresh game with empty prior history and freshly initialized
rule counters. It is intended for analysis positions, not exact live-game
resume. To preserve repetition/rule history, use `new` and replay `play`
commands. A versioned full-state snapshot/restore contract is planned before
v2.0.

## Commands while search is active

Only lifecycle commands are accepted:

```text
ping [TOKEN]
stop
quit
```

Other commands return `error busy`.

`stop` on an active search returns `ok stop`; repeated stop requests remain
idempotent for that search. `stop` while idle returns `ok idle`.

Cancellation is scoped to one `go/analyze`. If a stop arrives after the worker
has already completed but before its result event is processed, it still changes
only that completed search token and cannot stop the next search.

## Search results

A move result is:

```text
bestmove FROM:TO:CAPTURES score S depth D nodes N time_us T stop REASON
```

`CAPTURES` uses the same `-`/comma-list format. Stable stop reason tokens are:

```text
completed
time_limit
node_limit
external_stop
history_limit
proven_win_or_loss
```

Draw-claim, terminal and adjudication results use distinct lines (`claim`,
`claim_move`, terminal status, `adjudication ...`) rather than inventing a board
move.

## Lifecycle

`ping X` returns `pong X` even during search. `quit` requests stop if necessary,
waits for the worker to return ownership of the engine, emits `bye` and exits.

## Laboratory compatibility

Since v1.11.1, match.py and tune_search.py accept both this protocol and the
legacy examples/classic_server packed-move protocol. reference_lab.py requires
classic_engine. A packed RDOPEN2 opening is translated to public MoveSpec by
the client; mixed-protocol games resolve canonical legacy metadata via legal.
