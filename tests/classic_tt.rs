use russian_draughts_classic::{ClassicKey, Game, Move, TranspositionTable, TtBound};

#[test]
fn classic_tt_round_trips_verified_key_and_payload() {
    let game = Game::initial();
    let key = ClassicKey::from_game(&game);
    let hash = key.hash(game.zobrist().0);
    let mut tt = TranspositionTable::new(1);
    tt.new_search();

    tt.store(key, hash, 7, 2, 123, TtBound::Exact, Move::EMPTY);
    let hit = tt.probe(key, hash, 2).expect("stored entry must be found");
    assert_eq!(hit.depth, 7);
    assert_eq!(hit.extensions_left, 2);
    assert!(hit.extension_compatible);
    assert_eq!(hit.score, 123);
    assert_eq!(hit.bound, TtBound::Exact);
}

#[test]
fn incompatible_extension_budget_is_only_a_move_hint() {
    let game = Game::initial();
    let key = ClassicKey::from_game(&game);
    let hash = key.hash(game.zobrist().0);
    let mut tt = TranspositionTable::new(1);
    tt.new_search();

    let mut moves = [Move::EMPTY; russian_draughts_classic::DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = russian_draughts_classic::generate_moves(game.position(), &mut moves);
    assert!(len > 0);
    let best = moves[0];

    tt.store(key, hash, 9, 0, 222, TtBound::Exact, best);
    let incompatible = tt
        .probe(key, hash, 2)
        .expect("same position should remain an ordering hint");
    assert_eq!(incompatible.best_move, best);
    assert_eq!(incompatible.extensions_left, 0);
    assert!(!incompatible.extension_compatible);

    tt.store(key, hash, 7, 2, -17, TtBound::Lower, best);
    let compatible = tt
        .probe(key, hash, 2)
        .expect("matching extension budget must be preferred");
    assert!(compatible.extension_compatible);
    assert_eq!(compatible.extensions_left, 2);
    assert_eq!(compatible.depth, 7);
    assert_eq!(compatible.score, -17);
}

#[test]
fn classic_tt_entry_size_does_not_grow_when_extension_budget_is_added() {
    // v1.0.x used 56-byte entries. Extension compatibility is packed into the
    // old depth field so TT capacity does not regress.
    let tt = TranspositionTable::new(1);
    assert_eq!(TranspositionTable::entry_size_bytes(), 56);
    assert!(tt.capacity_entries() > 0);
    assert!(tt.allocated_bytes() <= 1024 * 1024);
}

#[test]
fn classic_key_changes_with_draw_rule_context() {
    let game = Game::initial();
    let key0 = ClassicKey::from_game(&game);

    let mut advanced = game.clone();
    let mut moves = [Move::EMPTY; russian_draughts_classic::DEFAULT_MOVE_BUFFER_CAPACITY];
    let len = russian_draughts_classic::generate_moves(advanced.position(), &mut moves);
    assert!(len > 0);
    advanced.play(moves[0]);
    let key1 = ClassicKey::from_game(&advanced);
    assert_ne!(key0, key1);
}

#[test]
fn classic_key_stays_compact() {
    assert_eq!(std::mem::size_of::<ClassicKey>(), 40);
}
