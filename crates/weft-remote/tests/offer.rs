//! remote.md §7a, §12 (ADR-0023, ADR-0024): turning Remote on lists the
//! tunnel adapters until one is chosen; with its program missing there is one
//! Install, which uses Homebrew when it is there, else the release for this
//! machine; it runs only when asked, and the tunnel starts once it is there.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{ADAPTER, CLOUDFLARED, Fake, Harness};
use serde_json::json;
use sha2::{Digest, Sha256};
use weft_remote::install::machine;

/// Take the configured program away, keeping it at `<path>.later`.
fn hide(dir: &Path) -> (PathBuf, PathBuf) {
    let program = dir.join("cloudflared");
    let later = dir.join("cloudflared.later");
    std::fs::rename(&program, &later).unwrap();
    (later, program)
}

/// The adapter with an install: Homebrew's argv (`brew`) and a release asset.
fn installable(dir: &Path, brew: &str) {
    std::fs::write(
        dir.join("tools/cloudflared.toml"),
        format!(
            "{ADAPTER}[install.brew]\nargv = [\"{brew}\", \"install\", \"cloudflared\"]\n\
             [install.download]\nrepo = \"cloudflare/cloudflared\"\n[install.download.assets]\n\"{}\" = \"cloudflared-test\"\n",
            machine()
        ),
    )
    .unwrap();
}

/// A brew that says what it does, then puts the program back where the config says.
fn fake_brew(dir: &Path, later: &Path, program: &Path) -> PathBuf {
    let brew = dir.join("brew");
    std::fs::write(
        &brew,
        format!(
            "#!/bin/sh\necho \"==> Downloading $2\"\nsleep 1\necho \"==> Pouring $2\"\nmv \"{}\" \"{}\"\n",
            later.display(),
            program.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    brew
}

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
    assert_eq!(h.tui.view["tunnels"], json!([{"id": "cloudflared", "title": "Cloudflare Tunnel"}]));
    h.tui.act(json!({"act": "choose", "tunnel": "cloudflared"}));
    let on = h.tui.wait_view(|v| v["state"] == "on" && v["tunnel"] == "up");
    assert_eq!(on["address"], "https://test-tunnel.trycloudflare.com");
    assert_eq!(on["note"], "traffic is decrypted at Cloudflare's edge", "the adapter's note");
    let config = std::fs::read_to_string(h.dir.join("config.toml")).unwrap();
    assert!(config.contains("tunnel = \"cloudflared\""), "{config}");
}

#[test]
fn install_uses_brew_when_it_is_there_and_runs_only_when_asked() {
    let mut brew = None;
    let mut h = Harness::until(
        "install-brew",
        |dir| {
            let (later, program) = hide(dir);
            let b = fake_brew(dir, &later, &program);
            installable(dir, &b.to_string_lossy());
            brew = Some(b);
        },
        "not_installed",
    );
    let brew = brew.unwrap();
    assert_eq!(
        h.tui.view["tunnel"],
        json!({"id": "cloudflared", "title": "Cloudflare Tunnel", "program": "cloudflared"})
    );
    assert_eq!(
        h.tui.view["offer"],
        json!({"how": "brew", "argv": format!("{} install cloudflared", brew.display())})
    );
    // Nothing runs until asked.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(h.dir.join("cloudflared.later").exists());

    h.tui.act(json!({"act": "install"}));
    let installing = h.tui.wait_view(|v| v["state"] == "installing");
    assert!(
        installing["line"].as_str().unwrap_or_default().contains("cloudflared"),
        "{installing}"
    );
    let on = h.tui.wait_view(|v| v["state"] == "on" && v["tunnel"] == "up");
    assert_eq!(on["address"], "https://test-tunnel.trycloudflare.com");
}

#[test]
fn without_brew_install_downloads_the_release_for_this_machine() {
    let github = Fake::start();
    let hex = |b: &[u8]| Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect::<String>();
    let program = CLOUDFLARED.as_bytes();
    github.route(
        "GET",
        "/repos/cloudflare/cloudflared/releases/latest",
        &[(200, &json!({
            "tag_name": "2026.9.3",
            "body": format!("### SHA256 Checksums:\ncloudflared-test: {}\n", hex(program)),
            "assets": [{"name": "cloudflared-test", "digest": format!("sha256:{}", hex(program)),
                        "browser_download_url": format!("{}/download/cloudflared-test", github.url)}],
        })
        .to_string())],
    );
    github.route_bytes("GET", "/download/cloudflared-test", 200, program.to_vec());
    let mut h = Harness::until_github(
        "install-download",
        |dir| {
            hide(dir);
            installable(dir, "/nonexistent/brew");
        },
        "not_installed",
        &github.url,
    );
    assert_eq!(
        h.tui.view["offer"],
        json!({"how": "download", "from": "github.com/cloudflare/cloudflared"})
    );
    h.tui.act(json!({"act": "install"}));
    let on = h.tui.wait_view(|v| v["state"] == "on" && v["tunnel"] == "up");
    assert_eq!(on["address"], "https://test-tunnel.trycloudflare.com");
    let config = std::fs::read_to_string(h.dir.join("config.toml")).unwrap();
    assert!(
        config.contains(&format!("program = \"{}\"", h.dir.join("bin/cloudflared").display())),
        "{config}"
    );
}

#[test]
fn a_download_that_fails_its_check_says_why_and_stays_off() {
    let github = Fake::start();
    github.route(
        "GET",
        "/repos/cloudflare/cloudflared/releases/latest",
        &[(200, &json!({
            "tag_name": "2026.9.3",
            "body": "### SHA256 Checksums:\ncloudflared-test: 00\n",
            "assets": [{"name": "cloudflared-test", "digest": format!("sha256:{}", "0".repeat(64)),
                        "browser_download_url": format!("{}/download/cloudflared-test", github.url)}],
        })
        .to_string())],
    );
    github.route_bytes("GET", "/download/cloudflared-test", 200, b"not it".to_vec());
    let mut h = Harness::until_github(
        "install-refused",
        |dir| {
            hide(dir);
            installable(dir, "/nonexistent/brew");
        },
        "not_installed",
        &github.url,
    );
    h.tui.act(json!({"act": "install"}));
    let refused = h.tui.wait_view(|v| v["failed"].is_string());
    assert_eq!(refused["state"], "not_installed");
    assert!(refused["failed"].as_str().unwrap().contains("GitHub's checksum"), "{refused}");
    assert!(!h.dir.join("bin/cloudflared").exists());
}

#[test]
fn an_adapter_with_no_install_says_why() {
    let h = Harness::until("install-none", |dir| drop(hide(dir)), "not_installed");
    assert!(h.tui.view["offer"].is_null());
    assert!(
        h.tui.view["why"].as_str().unwrap_or_default().contains("[remote] program"),
        "{}",
        h.tui.view
    );
}
