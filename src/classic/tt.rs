use std::mem::size_of;

use crate::game::PositionKey;
use crate::hash::mix64;
use crate::{Game, Move};

pub const TT_WAYS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Bound {
    #[default]
    None = 0,
    Exact = 1,
    Lower = 2,
    Upper = 3,
}

/// Classical search identity. Unlike the AlphaZero search key, the classical
/// evaluator does not consume ordered neural-history planes, so they are not
/// part of the TT identity. Repetition multiplicity and all draw-rule counters
/// remain included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct ClassicKey {
    position_lo: u64,
    position_hi: u64,
    repetition_lo: u64,
    repetition_hi: u64,
    rule_state: u64,
}

impl ClassicKey {
    #[inline(always)]
    pub fn from_game(game: &Game) -> Self {
        let position = PositionKey::from_position(game.position()).0;
        let repetition = game.history().repetition_context();
        Self {
            position_lo: position as u64,
            position_hi: (position >> 64) as u64,
            repetition_lo: repetition as u64,
            repetition_hi: (repetition >> 64) as u64,
            rule_state: game.packed_rule_state(),
        }
    }

    #[inline(always)]
    pub fn hash(self, position_hash: u64) -> u64 {
        mix64(
            position_hash
                ^ mix64(self.rule_state)
                ^ self.repetition_lo.rotate_left(23)
                ^ self.repetition_hi.rotate_right(7),
        )
    }
}

/// `depth_and_extensions` deliberately packs two search conditions into the
/// same 16 bits previously occupied by `depth`: low 8 bits are nominal depth,
/// high 8 bits are the remaining extension budget. This keeps `TtEntry` at 56
/// bytes, so fixing extension compatibility does not reduce TT capacity.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct TtEntry {
    key: ClassicKey,
    best_move: Move,
    score: i32,
    depth_and_extensions: u16,
    bound: Bound,
    generation: u8,
}

impl TtEntry {
    pub const EMPTY: Self = Self {
        key: ClassicKey {
            position_lo: 0,
            position_hi: 0,
            repetition_lo: 0,
            repetition_hi: 0,
            rule_state: 0,
        },
        best_move: Move::EMPTY,
        score: 0,
        depth_and_extensions: 0,
        bound: Bound::None,
        generation: 0,
    };

    #[inline(always)]
    const fn depth(self) -> i16 {
        (self.depth_and_extensions & 0x00FF) as i16
    }

    #[inline(always)]
    const fn extensions_left(self) -> u8 {
        (self.depth_and_extensions >> 8) as u8
    }

    #[inline(always)]
    const fn pack_depth_extensions(depth: i16, extensions_left: u8) -> u16 {
        let d = if depth <= 0 {
            0
        } else if depth >= u8::MAX as i16 {
            u8::MAX
        } else {
            depth as u8
        };
        (d as u16) | ((extensions_left as u16) << 8)
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct TtBucket {
    entries: [TtEntry; TT_WAYS],
}

impl TtBucket {
    const EMPTY: Self = Self {
        entries: [TtEntry::EMPTY; TT_WAYS],
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TtHit {
    pub best_move: Move,
    pub score: i32,
    pub depth: i16,
    pub bound: Bound,
    /// Remaining extension budget under which this score was searched.
    pub extensions_left: u8,
    /// True only when `extensions_left` matches the requested search. An
    /// incompatible hit is still useful as a move-ordering hint, but its score
    /// and bound must never be used for a cutoff.
    pub extension_compatible: bool,
}

pub struct TranspositionTable {
    buckets: Vec<TtBucket>,
    mask: usize,
    generation: u8,
}

impl TranspositionTable {
    pub fn new(megabytes: usize) -> Self {
        let requested = megabytes.max(1).saturating_mul(1024 * 1024);
        let bucket_bytes = size_of::<TtBucket>().max(1);
        let raw = (requested / bucket_bytes).max(1);
        let bucket_count = floor_power_of_two(raw);
        let buckets = vec![TtBucket::EMPTY; bucket_count];
        Self {
            buckets,
            mask: bucket_count - 1,
            generation: 1,
        }
    }

    pub fn clear(&mut self) {
        self.buckets.fill(TtBucket::EMPTY);
        self.generation = 1;
    }

    pub fn new_search(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            // A full generation wrap is rare, but clearing avoids making very
            // old entries look fresh to the replacement policy.
            self.clear();
        }
    }

    #[inline(always)]
    fn bucket_index(&self, hash: u64) -> usize {
        hash as usize & self.mask
    }

    /// Probe one bucket. If an entry with the exact extension budget exists it
    /// is preferred. Otherwise a same-position entry is returned only as an
    /// incompatible move-ordering hint.
    #[inline]
    pub fn probe(&self, key: ClassicKey, hash: u64, extensions_left: u8) -> Option<TtHit> {
        let bucket = &self.buckets[self.bucket_index(hash)];
        let mut hint = None;
        for entry in bucket.entries {
            if entry.generation == 0 || entry.key != key {
                continue;
            }
            let compatible = entry.extensions_left() == extensions_left;
            let hit = TtHit {
                best_move: entry.best_move,
                score: entry.score,
                depth: entry.depth(),
                bound: entry.bound,
                extensions_left: entry.extensions_left(),
                extension_compatible: compatible,
            };
            if compatible {
                return Some(hit);
            }
            if hint.is_none() {
                hint = Some(hit);
            }
        }
        hint
    }

    // TT stores a compact search record. Keeping the fields explicit at the call
    // site makes replacement semantics auditable and avoids constructing a
    // temporary public payload solely to satisfy this style lint.
    #[allow(clippy::too_many_arguments)]
    pub fn store(
        &mut self,
        key: ClassicKey,
        hash: u64,
        depth: i16,
        extensions_left: u8,
        score: i32,
        bound: Bound,
        best_move: Move,
    ) {
        let generation = self.generation;
        let index = self.bucket_index(hash);
        let bucket = &mut self.buckets[index];
        let packed = TtEntry::pack_depth_extensions(depth, extensions_left);

        // Only an entry produced under the same extension budget is the same
        // search condition. Different budgets are allowed to coexist in
        // separate ways of the bucket.
        for entry in &mut bucket.entries {
            if entry.generation != 0
                && entry.key == key
                && entry.extensions_left() == extensions_left
            {
                if depth >= entry.depth() || bound == Bound::Exact || entry.generation != generation
                {
                    *entry = TtEntry {
                        key,
                        best_move,
                        score,
                        depth_and_extensions: packed,
                        bound,
                        generation,
                    };
                } else {
                    // Even a shallower revisit refreshes age and may provide a
                    // useful current best move for ordering.
                    entry.generation = generation;
                    if best_move != Move::EMPTY {
                        entry.best_move = best_move;
                    }
                }
                return;
            }
        }

        if let Some(empty) = bucket.entries.iter_mut().find(|e| e.generation == 0) {
            *empty = TtEntry {
                key,
                best_move,
                score,
                depth_and_extensions: packed,
                bound,
                generation,
            };
            return;
        }

        let mut replace = 0usize;
        let mut lowest = replacement_quality(bucket.entries[0], generation);
        for i in 1..TT_WAYS {
            let quality = replacement_quality(bucket.entries[i], generation);
            if quality < lowest {
                lowest = quality;
                replace = i;
            }
        }
        bucket.entries[replace] = TtEntry {
            key,
            best_move,
            score,
            depth_and_extensions: packed,
            bound,
            generation,
        };
    }

    pub const fn entry_size_bytes() -> usize {
        size_of::<TtEntry>()
    }

    pub fn capacity_entries(&self) -> usize {
        self.buckets.len() * TT_WAYS
    }

    pub fn allocated_bytes(&self) -> usize {
        self.buckets.len() * size_of::<TtBucket>()
    }

    /// Approximate occupancy of the current generation in permille. Sampling
    /// keeps this diagnostic cheap regardless of TT size.
    pub fn hashfull_per_mille(&self) -> u16 {
        let sample_buckets = self.buckets.len().min(250);
        if sample_buckets == 0 {
            return 0;
        }
        let mut used = 0usize;
        for bucket in &self.buckets[..sample_buckets] {
            for entry in bucket.entries {
                used += (entry.generation == self.generation) as usize;
            }
        }
        let total = sample_buckets * TT_WAYS;
        ((used * 1000) / total) as u16
    }
}

#[inline(always)]
fn replacement_quality(entry: TtEntry, current_generation: u8) -> i32 {
    let age = if entry.generation == current_generation {
        256
    } else {
        0
    };
    let exact = if entry.bound == Bound::Exact { 32 } else { 0 };
    age + exact + entry.depth().max(0) as i32
}

fn floor_power_of_two(value: usize) -> usize {
    debug_assert!(value > 0);
    1usize << (usize::BITS - 1 - value.leading_zeros())
}
