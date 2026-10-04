use russian_draughts_classic::{ENGINE_ID, ENGINE_PACKAGE, ENGINE_VERSION};

#[test]
fn public_version_is_derived_from_cargo_package_metadata() {
    assert_eq!(ENGINE_PACKAGE, env!("CARGO_PKG_NAME"));
    assert_eq!(ENGINE_VERSION, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        ENGINE_ID,
        concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"))
    );
}
