//! remote.md §7a, §12: turning Remote on lists the tunnel adapters, each
//! with its guide, until one is chosen; installing its program is the
//! person's: with it missing, the view names the program and its guide, and
//! Check (`look`) starts the tunnel once it is there.

mod common;

use common::{GUIDE, Harness};
use serde_json::json;

#[test]
fn with_no_tunnel_chosen_the_adapters_are_listed_and_a_choice_is_kept() {
    let mut h = Harness::until(
        "choose",
        |dir| {
            let config = std::fs::read_to_string(dir.join("config.toml")).unwrap();
            let config: String = config
                .lines()
                .filter(|l| !l.starts_with("tunnel"))
                .map(|l| format!("{l}\n"))
                .collect();
            std::fs::write(dir.join("config.toml"), config).unwrap();
            std::fs::write(dir.join("tools/push.toml"), "kind = \"push\"\nprogram = \"p\"\n")
                .unwrap();
        },
        "choose",
    );
    assert_eq!(
        h.app.view["tunnels"],
        json!([{"id": "cloudflared", "title": "Cloudflare Tunnel",
                "note": "traffic is decrypted at Cloudflare's edge", "guide": GUIDE}])
    );
    h.app.act(json!({"act": "choose", "tunnel": "cloudflared"}));
    let on = h.app.wait_view(|v| v["state"] == "on" && v["tunnel"] == "up");
    assert_eq!(on["address"], "https://test-tunnel.trycloudflare.com");
    assert_eq!(on["note"], "traffic is decrypted at Cloudflare's edge", "the adapter's note");
    let config = std::fs::read_to_string(h.dir.join("config.toml")).unwrap();
    assert!(config.contains("tunnel = \"cloudflared\""), "{config}");
}

/// Installing its program is the person's: missing, the view names it and
/// its guide; Check starts the tunnel once it is there.
#[test]
fn a_missing_program_is_named_with_its_guide_and_check_starts_it_once_there() {
    let mut h = Harness::until(
        "missing",
        |dir| std::fs::rename(dir.join("cloudflared"), dir.join("cloudflared.later")).unwrap(),
        "not_installed",
    );
    assert_eq!(
        h.app.view["tunnel"],
        json!({"id": "cloudflared", "title": "Cloudflare Tunnel", "program": "cloudflared",
               "guide": GUIDE})
    );
    assert!(h.app.view.get("offer").is_none(), "nothing for Weft to install: {}", h.app.view);
    std::fs::rename(h.dir.join("cloudflared.later"), h.dir.join("cloudflared")).unwrap();
    h.app.act(json!({"act": "look"}));
    let on = h.app.wait_view(|v| v["state"] == "on" && v["tunnel"] == "up");
    assert_eq!(on["address"], "https://test-tunnel.trycloudflare.com");
}

/// Until the tunnel gives its address, the last line its program said is
/// shown as it said it; with one adapter there is nothing to change to.
#[test]
fn a_tunnel_that_has_not_started_says_why() {
    let mut h = Harness::with("tunnel-why", |dir| {
        let program = dir.join("cloudflared");
        let script = "#!/bin/sh\necho \"$*\" > \"$0.args\"\necho 'Funnel is not enabled on your tailnet.'\necho\nexit 1\n";
        std::fs::write(&program, script).unwrap();
    });
    let why = h.app.wait_view(|v| v["tunnel_why"].is_string());
    assert_eq!(why["tunnel"], "starting");
    assert_eq!(why["tunnel_why"], "Funnel is not enabled on your tailnet.");
    assert_eq!(why["tunnel_title"], "Cloudflare Tunnel", "whose words they are");
    assert!(why["can_change_tunnel"].is_null(), "one adapter: {why}");
}

/// With two adapters, Change tunnel stops the running one and lists both,
/// the current one chosen; picking the other forgets where the first's
/// program was.
#[test]
fn change_tunnel_stops_the_tunnel_and_switching_forgets_its_program() {
    let mut h = Harness::with("change-tunnel", |dir| {
        let other = "kind = \"tunnel\"\ntitle = \"Other Tunnel\"\nprogram = \"no-such-other-tunnel\"\nrun = [\"{port}\"]\naddress = \".other.example\"\n";
        std::fs::write(dir.join("tools/other.toml"), other).unwrap();
    });
    let on = h.app.wait_view(|v| v["tunnel"] == "up");
    assert_eq!(on["can_change_tunnel"], true);
    assert!(h.dir.join("remote/tunnel.pid").exists());
    h.app.act(json!({"act": "change_tunnel"}));
    let choose = h.app.wait_view(|v| v["state"] == "choose");
    assert_eq!(choose["chosen"], "cloudflared");
    let ids: Vec<_> =
        choose["tunnels"].as_array().unwrap().iter().map(|t| t["id"].clone()).collect();
    assert_eq!(ids, [json!("cloudflared"), json!("other")]);
    assert!(!h.dir.join("remote/tunnel.pid").exists(), "the running tunnel stopped");
    h.app.act(json!({"act": "choose", "tunnel": "other"}));
    let missing = h.app.wait_view(|v| v["state"] == "not_installed");
    assert_eq!(missing["tunnel"]["id"], "other");
    let config = std::fs::read_to_string(h.dir.join("config.toml")).unwrap();
    assert!(config.contains("tunnel = \"other\""), "{config}");
    assert!(!config.contains("program = \"/"), "the first's program is forgotten: {config}");
}
