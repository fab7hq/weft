//! remote.md §12: with the tunnel's program missing, the view says so;
//! looking again once it is there starts the tunnel.

mod common;

use common::Harness;
use serde_json::json;

#[test]
fn looking_again_finds_cloudflared_once_it_is_installed() {
    let mut aside = None;
    let mut h = Harness::until(
        "look",
        |dir| {
            let program = dir.join("cloudflared");
            let moved = dir.join("cloudflared.later");
            std::fs::rename(&program, &moved).unwrap();
            aside = Some((moved, program));
        },
        "not_installed",
    );
    assert_eq!(h.app.view["tunnel"]["title"], "Cloudflare Tunnel");
    let (moved, program) = aside.unwrap();
    std::fs::rename(moved, program).unwrap();
    h.app.act(json!({"act": "look"}));
    let on = h.app.wait_view(|v| v["state"] == "on" && v["tunnel"] == "up");
    assert_eq!(on["address"], "https://test-tunnel.trycloudflare.com");
}
