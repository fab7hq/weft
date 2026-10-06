//! A machine removed from the account on the web (D1.6) joins again: when the
//! service says it is not on an account, weft remote forgets its place and
//! signs in, as a machine that never joined does (W → R shows the code).

mod common;

use common::{Fake, Harness};

#[test]
fn a_machine_removed_from_the_account_signs_in_again() {
    let service = Fake::start();
    service.route(
        "PUT",
        "/api/machine/address",
        &[(401, r#"{"code":"unknown_machine","message":"This machine is not on an account."}"#)],
    );
    let login = Fake::start();
    login.route(
        "POST",
        "/login/device/code",
        &[(200, r#"{"device_code":"dev-1","user_code":"AB-CD","verification_uri":"https://github.com/login/device","interval":5}"#)],
    );
    login.route(
        "POST",
        "/login/oauth/access_token",
        &[(200, r#"{"error":"authorization_pending"}"#)],
    );
    let mut h =
        Harness::until_at("left", |_| {}, "on", "http://127.0.0.1:9", &service.url, &login.url);
    let view = h.tui.wait_view(|v| v["code"] == "AB-CD");
    assert!(view["login"].is_null(), "no longer on octo's account: {view}");
    assert!(!h.dir.join("remote/machine.toml").exists(), "its old place is forgotten");
    assert_eq!(service.requests("PUT", "/api/machine/address").len(), 1);
}
