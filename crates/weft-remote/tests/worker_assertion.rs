//! account.md §7: an assertion the Worker signed (weft-app's own `sign`, with
//! its test key) verifies here, byte for byte, and only for what it names.

use weft_remote::devices;

#[test]
fn an_assertion_the_worker_signed_verifies_here() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/worker-assertion.json")).unwrap();
    let key = devices::key(fixture["public"].as_str().unwrap()).expect("a key");
    let assertion = fixture["assertion"].as_str().unwrap();
    let now = fixture["now"].as_i64().unwrap();
    let a = devices::check(&key, assertion, now, "m_fixture", 42).expect("it verifies");
    assert_eq!((a.login.as_str(), a.device.as_str(), a.exp), ("octo", "d_fixture", now + 30));
    assert_eq!(devices::check(&key, assertion, now + 30, "m_fixture", 42), Err("expired"));
    assert_eq!(devices::check(&key, assertion, now, "m_other", 42), Err("another machine"));
    assert!(
        devices::check(&devices::fab7_key(), assertion, now, "m_fixture", 42).is_err(),
        "the test key is not Fab7's"
    );
}
