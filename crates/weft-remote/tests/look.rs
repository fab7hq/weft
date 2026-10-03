//! remote.md §12: with no `cloudflared`, the view says how to install it;
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
        "no_cloudflared",
    );
    assert_eq!(h.tui.view["install"], "brew install cloudflared");
    let (moved, program) = aside.unwrap();
    std::fs::rename(moved, program).unwrap();
    h.tui.act(json!({"act": "look"}));
    let on = h.tui.wait_view(|v| v["state"] == "on" && v["tunnel"] == "up");
    assert_eq!(on["address"], "https://test-tunnel.trycloudflare.com");
}
