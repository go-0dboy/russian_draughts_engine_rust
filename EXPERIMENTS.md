# Experiment log

Only accepted changes belong to the production baseline. Rejected experiments
are recorded here so the same ideas are not repeatedly reintroduced without new
evidence.

## Accepted baseline lineage

- v1.5.1 — accepted tuned search/HCE baseline and single-source SearchParams.
- v1.7.0 — accepted production baseline: match-process hardening, lost-stop race
  fix and terminal-state precedence regression; strength heuristics remain from
  the accepted v1.5.1 line.

## Rejected / not promoted

### v1.6.x staged MovePicker experiments

- v1.6.0: exact search-tree equivalence; roughly +3% local NPS; equal-time
  strength was statistically neutral.
- v1.6.3: bitset/lazy variant; exact tree but ~6% slower than v1.6.0; rejected.
- v1.6.4: exact tree and ~5% corpus NPS over v1.5.1, but no demonstrated
  strength improvement. Kept as an experiment, not production.

### v1.7.1 SearchWorkspace reuse

Exact fixed-node semantics, but no reproducible timing benefit:
median about -1.08%, mean about +0.55% with high noise. Rejected.

### v1.8.0 main/capture/continuation history

Long fixed-node match versus v1.7.0:

```text
A wins / draws / B wins: 646 / 2704 / 639
A score: 0.500754
LLR: +0.269399
A nodes/move: 45572.9
B nodes/move: 45550.0
```

Statistically neutral and therefore rejected. Do not tune this exact design
further without diagnostics showing a concrete move-ordering weakness.

## Experiment policy

Unaccepted hypotheses use branch/experiment names, not release numbers. A
search-strength candidate is promoted only after:

`correctness -> diagnostics -> fixed-node GSPRT -> equal-time GSPRT`.
