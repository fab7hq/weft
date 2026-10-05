//! `words.json`, the web app's language pack, is the words table as it is.

/// Kept beside this crate and copied into the web app.
/// `WEFT_WRITE_WORDS=1 cargo test -p weft-core --test words_json` writes it again.
#[test]
fn words_json_is_the_table() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("words.json");
    if std::env::var_os("WEFT_WRITE_WORDS").is_some() {
        std::fs::write(&path, weft_core::words::json()).unwrap();
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        kept == weft_core::words::json(),
        "words.json is stale: WEFT_WRITE_WORDS=1 cargo test -p weft-core --test words_json"
    );
}
