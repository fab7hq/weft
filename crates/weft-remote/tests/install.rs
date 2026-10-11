//! Installing cloudflared by download (remote.md §7a): Cloudflare's
//! latest release, checked against GitHub's digest and the release's notes,
//! against a fake GitHub.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::Fake;
use sha2::{Digest, Sha256};
use weft_remote::install::{Tool, download, machine};

const PROGRAM: &[u8] = b"#!/bin/sh\necho a fake cloudflared\n";

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn tool(asset: &str) -> Tool {
    Tool {
        id: "cloudflared".into(),
        kind: "tunnel".into(),
        title: "Cloudflare Tunnel".into(),
        program: "cloudflared".into(),
        repo: Some("cloudflare/cloudflared".into()),
        assets: BTreeMap::from([(machine(), asset.to_string())]),
        ..Tool::default()
    }
}

/// A .tgz holding the program at its top, as Cloudflare's macOS assets do.
fn tgz(dir: &Path) -> Vec<u8> {
    let inner = dir.join("pack");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(inner.join("cloudflared"), PROGRAM).unwrap();
    let out = dir.join("cf.tgz");
    let ok = std::process::Command::new("tar")
        .args(["-czf", &out.to_string_lossy(), "-C", &inner.to_string_lossy(), "cloudflared"])
        .status()
        .unwrap()
        .success();
    assert!(ok);
    std::fs::read(out).unwrap()
}

/// GitHub's latest release with one asset, its digest and the notes' line.
fn release(fake: &Fake, asset: &str, bytes: &[u8], digest: &str, notes: &str) {
    let body = serde_json::json!({
        "tag_name": "2026.9.3",
        "body": format!("### Changes\nfixes\n\n### SHA256 Checksums:\n{notes}\n"),
        "assets": [{
            "name": asset,
            "browser_download_url": format!("{}/download/{asset}", fake.url),
            "digest": digest,
        }],
    });
    fake.route("GET", "/repos/cloudflare/cloudflared/releases/latest", &[(200, &body.to_string())]);
    fake.route_bytes("GET", &format!("/download/{asset}"), 200, bytes.to_vec());
}

/// A `~/.fab7/weft` of its own, removed when dropped.
struct Weft(std::path::PathBuf);

impl Weft {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Weft {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn weft() -> Weft {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("wr-install-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "[remote]\nenabled = true\n").unwrap();
    Weft(dir)
}

#[test]
fn a_program_asset_is_checked_installed_and_named_in_the_config() {
    let (fake, weft) = (Fake::start(), weft());
    let asset = "cloudflared-test-amd64";
    release(
        &fake,
        asset,
        PROGRAM,
        &format!("sha256:{}", hex(PROGRAM)),
        &format!("{asset}: {}", hex(PROGRAM)),
    );
    let path = download(&fake.url, &tool(asset), weft.path()).unwrap();
    assert_eq!(path, weft.path().join("bin/cloudflared"));
    assert_eq!(std::fs::read(&path).unwrap(), PROGRAM);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o755);
    let config = std::fs::read_to_string(weft.path().join("config.toml")).unwrap();
    assert!(config.contains(&format!("program = \"{}\"", path.display())), "{config}");
    assert!(config.contains("enabled = true"), "the rest kept: {config}");
    // GitHub's API wants a user agent.
    let asked = fake.requests("GET", "/repos/cloudflare/cloudflared/releases/latest");
    assert!(asked[0].headers.contains_key("user-agent"));
}

#[test]
fn a_tgz_asset_is_unpacked_and_its_program_checked_against_the_notes() {
    let (fake, weft) = (Fake::start(), weft());
    let asset = "cloudflared-test-arm64.tgz";
    let packed = tgz(weft.path());
    // The notes give the program's checksum; GitHub's digest is the archive's.
    release(
        &fake,
        asset,
        &packed,
        &format!("sha256:{}", hex(&packed)),
        &format!("{asset}: {}", hex(PROGRAM)),
    );
    let path = download(&fake.url, &tool(asset), weft.path()).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), PROGRAM);
}

#[test]
fn a_wrong_digest_installs_nothing() {
    let (fake, weft) = (Fake::start(), weft());
    let asset = "cloudflared-test-amd64";
    release(
        &fake,
        asset,
        PROGRAM,
        &format!("sha256:{}", "0".repeat(64)),
        &format!("{asset}: {}", hex(PROGRAM)),
    );
    let why = download(&fake.url, &tool(asset), weft.path()).unwrap_err();
    assert!(why.contains("GitHub's checksum"), "{why}");
    assert!(!weft.path().join("bin/cloudflared").exists());
    assert!(
        !std::fs::read_to_string(weft.path().join("config.toml"))
            .unwrap()
            .contains("cloudflared =")
    );
}

#[test]
fn a_wrong_or_missing_notes_checksum_installs_nothing() {
    for notes in
        ["cloudflared-test-amd64.deb: 00", &format!("cloudflared-test-amd64: {}", "1".repeat(64))]
    {
        let (fake, weft) = (Fake::start(), weft());
        let asset = "cloudflared-test-amd64";
        release(&fake, asset, PROGRAM, &format!("sha256:{}", hex(PROGRAM)), notes);
        let why = download(&fake.url, &tool(asset), weft.path()).unwrap_err();
        assert!(why.contains("release notes"), "{notes}: {why}");
        assert!(!weft.path().join("bin/cloudflared").exists());
    }
}

#[test]
fn no_asset_for_this_machine_installs_nothing() {
    let (fake, weft) = (Fake::start(), weft());
    let mut t = tool("x");
    t.assets.clear();
    let why = download(&fake.url, &t, weft.path()).unwrap_err();
    assert!(why.contains(&machine()), "{why}");
    assert!(why.contains("Cloudflare Tunnel"), "named by its title: {why}");
    assert!(fake.requests("GET", "/repos/cloudflare/cloudflared/releases/latest").is_empty());
}
