//! The account client against a fake GitHub and service (account.md §3,
//! §5): the device flow's code reaches the view, the machine is enrolled
//! with GitHub's token which is not kept, and only a new address is sent.

mod common;

use std::sync::atomic::AtomicBool;

use common::Fake;
use weft_remote::account::{Account, Report, Reported, SignIn};
use weft_remote::store::Machine;
use weft_remote::view::View;

#[test]
fn signing_in_shows_the_code_then_joins_the_account() {
    let github = Fake::start();
    github.route(
        "POST",
        "/login/device/code",
        &[(200, r#"{"device_code":"dev-1","user_code":"WXYZ-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":0}"#)],
    );
    github.route(
        "POST",
        "/login/oauth/access_token",
        &[
            (200, r#"{"error":"authorization_pending"}"#),
            (200, r#"{"access_token":"ghu_once","token_type":"bearer"}"#),
        ],
    );
    let service = Fake::start();
    service.route(
        "POST",
        "/api/machine/enrol",
        &[(
            200,
            r#"{"machine_id":"m_1","machine_token":"mt_secret","github_id":42,"login":"octo"}"#,
        )],
    );

    let mut view = View::default();
    let mut joined = None;
    Account::at(&service.url, &github.url).sign_in("studio", &AtomicBool::new(false), |step| {
        match step {
            SignIn::Code { code, url } => {
                view.signing_in = Some((code, url));
                let shown = view.json();
                assert_eq!(shown["state"], "signing_in");
                assert_eq!(shown["code"], "WXYZ-1234");
                assert_eq!(shown["url"], "github.com/login/device");
            }
            SignIn::Joined(m) => joined = Some(m),
            SignIn::Failed(why) => panic!("{why}"),
        }
    });
    assert_eq!(
        joined,
        Some(Machine {
            id: "m_1".into(),
            name: "studio".into(),
            github_id: 42,
            login: "octo".into(),
            token: "mt_secret".into()
        })
    );
    let asked = github.requests("POST", "/login/device/code");
    assert_eq!(asked[0].text(), "client_id=Iv23liun7wYzjax5d6p3", "the public id, no secret");
    assert_eq!(
        github.requests("POST", "/login/oauth/access_token").len(),
        2,
        "polled until the yes"
    );
    let enrol = &service.requests("POST", "/api/machine/enrol")[0];
    let body: serde_json::Value = serde_json::from_slice(&enrol.body).unwrap();
    assert_eq!(body, serde_json::json!({"github_token": "ghu_once", "name": "studio"}));
}

#[test]
fn an_address_change_makes_exactly_one_put() {
    let service = Fake::start();
    service.route("PUT", "/api/machine/address", &[(204, "")]);
    let account = Account::at(&service.url, "http://127.0.0.1:9");
    let machine = Machine {
        id: "m_1".into(),
        name: "studio".into(),
        github_id: 42,
        login: "octo".into(),
        token: "mt_secret".into(),
    };
    let mut reported = Reported::default();
    for address in [
        "https://a-b.trycloudflare.com",
        "https://a-b.trycloudflare.com",
        "https://c-d.trycloudflare.com",
    ] {
        if reported.is_new(address) {
            account.report(&machine, address).unwrap();
        }
    }
    let puts = service.requests("PUT", "/api/machine/address");
    assert_eq!(puts.len(), 2, "one per change");
    assert_eq!(puts[0].headers["authorization"], "Bearer mt_secret");
    assert_eq!(puts[1].text(), r#"{"address":"https://c-d.trycloudflare.com"}"#);
}

#[test]
fn a_refused_sign_in_says_why() {
    let github = Fake::start();
    github.route(
        "POST",
        "/login/device/code",
        &[(200, r#"{"device_code":"dev-1","user_code":"AB-CD","verification_uri":"https://github.com/login/device","interval":0}"#)],
    );
    github.route("POST", "/login/oauth/access_token", &[(200, r#"{"error":"access_denied"}"#)]);
    let mut said = None;
    Account::at("http://127.0.0.1:9", &github.url).sign_in(
        "studio",
        &AtomicBool::new(false),
        |step| {
            if let SignIn::Failed(why) = step {
                said = Some(why);
            }
        },
    );
    assert_eq!(said.as_deref(), Some("GitHub said access_denied"));
}

/// A machine removed from the account on the web (D1.6) is told so when it
/// next reports an address, rather than the answer going unread.
#[test]
fn a_machine_removed_from_the_account_is_told_so() {
    let service = Fake::start();
    service.route(
        "PUT",
        "/api/machine/address",
        &[(401, r#"{"code":"unknown_machine","message":"This machine is not on an account."}"#)],
    );
    let account = Account::at(&service.url, "http://127.0.0.1:9");
    let machine = Machine {
        id: "m_1".into(),
        name: "studio".into(),
        github_id: 42,
        login: "octo".into(),
        token: "mt_gone".into(),
    };
    assert_eq!(
        account.report(&machine, "https://a-b.trycloudflare.com"),
        Ok(Report::NotOnAnAccount)
    );
    service.route("PUT", "/api/machine/address", &[(204, "")]);
    assert_eq!(account.report(&machine, "https://a-b.trycloudflare.com"), Ok(Report::Taken));
}
