# RDTUNE3 / format version 1

All integers little-endian, no native padding. STATIC only. White-perspective
raw HCE feature schema `hce-92-raw-white-v1`. Side-to-move is independent of label.
Result codes: 0 black win, 1 draw, 2 white win. Header size 28; record size 246.

Header `<8sIIQHBB>`: `RDTUNE3\0`, version=1, record_size=246, count(u64),
feature_count=92, role(u8), reserved=0. Roles: train=0, dev=1, reference=2,
arena=3, final=4.

Record prefix `<32sIIIBBHHBBBBd>` (62 bytes):
SHA256 semantic game ID; white/black/kings u32 bitboards; side u8; result u8;
ply u16; phase u16 (0..256); flags u8 (capture=1,promotion=2); piece count u8;
king count u8; reserved zero u8; weight IEEE754 binary64.
Then 92 signed i16 features. No raw game history or packed move is in a record.
Weights sum to one per retained game within a role. No qscore is fabricated;
mode/schema changes require a new compatible definition, not silently filling zero.

`ROLE.games.jsonl` stores root placement and full stable MoveSpec path (0..31,
exact captured-square bitset), result and game ID. `raw_moves` is explicitly a
legacy engine-specific compatibility cache for RDOPEN2; it is not used in game
identity or product replay. Its producing executable is hashed in the manifest.

`manifest.json` binds source PDN SHA256, exporter SHA256, Python builder/schema
SHA256, selection parameters, counts/drops, sample-class counts and SHA256 of
all output files. Original source path is not an identity. Manifest hash binds
teacher/training runs. Source reordering preserves semantic IDs/splits and data
record order; source checksum and rejection report can differ as expected.

The audit tool checks file hashes, record framing, cross-role exact-position
isolation and per-game weight sums. It does not prove chess/draughts reachability
from a board alone. Rust exporter and replay integration tests own rule correctness.

The reader refuses FINAL unless explicitly requested. This is an accidental-use
barrier, not encryption or proof of a blind holdout. FINAL from the previously
used human corpus is explicitly RETROSPECTIVE_NOT_BLIND.

Cross-role ownership: final > reference > arena > dev > train. Equal positions
within one role can occur in distinct games and retain separate outcomes/weights.
Repeated positions within one game are sampled once. Duplicate semantic games
are counted once; conflicting-result duplicate games are removed entirely.
