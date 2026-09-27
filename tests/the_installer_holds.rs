//! `install.sh` downloads and verifies; it never builds.
//!
//! This stubs `curl`, the only thing the script reaches the network with, and
//! serves a release made on the spot. Everything else — the platform
//! detection, the checksum, the unpacking, the install — is the real script.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Release {
    dir: tempfile::TempDir,
}

impl Release {
    /// A release containing both binaries, its checksum, and a `curl` that
    /// serves them from disk.
    fn make(tag: &str, corrupt_checksum: bool) -> Release {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let serve = dir.path().join("serve");
        std::fs::create_dir_all(&serve).unwrap();

        let target = format!("{}-{}", arch(), platform());
        let archive = format!("fab7-{tag}-{target}.tar.gz");
        let staged = dir.path().join("staged");
        std::fs::create_dir_all(&staged).unwrap();
        for name in ["weft", "ringframe"] {
            let p = staged.join(name);
            // `ringframe init --global` records the fab7 tag it installed, as
            // the real one does.
            let init = format!(
                "if [ \"${{1:-}}\" = init ]; then mkdir -p \"$HOME/.fab7/rf/config\" && \
                 echo {tag} > \"$HOME/.fab7/rf/config/.revision\"; fi\n"
            );
            std::fs::write(&p, format!("#!/bin/sh\n{init}echo \"{name} {tag}\"\n")).unwrap();
            make_executable(&p);
        }
        run(&[
            "tar",
            "-czf",
            &serve.join(&archive).to_string_lossy(),
            "-C",
            &staged.to_string_lossy(),
            "weft",
            "ringframe",
        ]);

        let digest = sha256_of(&serve.join(&archive));
        let written = if corrupt_checksum { "0".repeat(64) } else { digest };
        std::fs::write(serve.join(format!("{archive}.sha256")), format!("{written}  {archive}\n"))
            .unwrap();
        std::fs::write(serve.join("releases-latest"), format!("{{\"tag_name\": \"{tag}\"}}\n"))
            .unwrap();

        // fab7 at that tag, as GitHub serves it: one directory, holding
        // Weft's harness files.
        let fab7 = dir.path().join("fab7");
        let harnesses = fab7.join(format!("fab7-{tag}/products/weft/harnesses"));
        std::fs::create_dir_all(&harnesses).unwrap();
        std::fs::write(harnesses.join("zed-agent.toml"), "title = \"Zed Agent\"\n").unwrap();
        run(&[
            "tar",
            "-czf",
            &serve.join(tag).to_string_lossy(),
            "-C",
            &fab7.to_string_lossy(),
            &format!("fab7-{tag}"),
        ]);

        // A `curl -fsSL [-o OUT] URL` that reads from `serve/` by the last
        // path segment, and fails on anything it was not given.
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let curl = bin.join("curl");
        let stub = r#"#!/bin/sh
# A `curl -fsSL [-o OUT] URL` that serves SERVE_DIR by the URL's last segment.
set -eu
serve='SERVE_DIR'
out=''
url=''
while [ $# -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift 2 ;;
        -*) shift ;;
        *) url=$1; shift ;;
    esac
done
case "$url" in
    *releases/latest) name=releases-latest ;;
    *) name=${url##*/} ;;
esac
[ -f "$serve/$name" ] || exit 22
if [ -n "$out" ]; then cp "$serve/$name" "$out"; else cat "$serve/$name"; fi
"#;
        std::fs::write(&curl, stub.replace("SERVE_DIR", &serve.display().to_string())).unwrap();
        make_executable(&curl);
        Release { dir }
    }

    fn install(&self, tag: &str) -> std::process::Output {
        let bin_dir = self.dir.path().join("target-bin");
        let home = self.dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        Command::new("sh")
            .arg(script())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.dir.path().join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("WEFT_VERSION", tag)
            .env("WEFT_BIN_DIR", &bin_dir)
            .env("HOME", &home)
            .output()
            .expect("sh")
    }

    fn installed(&self, name: &str) -> PathBuf {
        self.dir.path().join("target-bin").join(name)
    }
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh")
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") { "apple-darwin" } else { "unknown-linux-musl" }
}

fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" }
}

fn make_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run(argv: &[&str]) {
    let out = Command::new(argv[0]).args(&argv[1..]).output().expect(argv[0]);
    assert!(out.status.success(), "{argv:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn sha256_of(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

#[test]
fn it_downloads_verifies_and_installs_both_binaries() {
    let release = Release::make("v0.1.0", false);
    let out = release.install("v0.1.0");
    let text =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    // `ringframe init --global` runs at the end and the stub is not the real
    // binary, so the script may end there; what matters is that both binaries
    // were verified and installed first.
    for name in ["weft", "ringframe"] {
        let p = release.installed(name);
        assert!(p.exists(), "{name} was not installed\n{text}");
        let shown = Command::new(&p).output().expect("the installed binary runs");
        assert_eq!(
            String::from_utf8_lossy(&shown.stdout).trim(),
            format!("{name} v0.1.0"),
            "{name} is not what the release contained"
        );
    }
    assert!(text.contains("Downloading fab7hq/weft v0.1.0"), "{text}");
}

/// Weft's harness files come with fab7 at the tag RingFrame's configuration
/// came from, and replace whatever was there: they are never edited.
#[test]
fn the_harness_files_are_installed_from_the_same_fab7_and_replace_the_old() {
    let release = Release::make("v0.1.0", false);
    let installed = release.dir.path().join("home/.fab7/weft/harnesses");
    std::fs::create_dir_all(&installed).unwrap();
    std::fs::write(installed.join("gone.toml"), "title = \"Gone\"\n").unwrap();
    let out = release.install("v0.1.0");
    let text =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(installed.join("zed-agent.toml").is_file(), "{text}");
    assert!(!installed.join("gone.toml").exists(), "replaced, not added to: {text}");
}

#[test]
fn a_bad_checksum_refuses_before_anything_is_unpacked() {
    let release = Release::make("v0.1.0", true);
    let out = release.install("v0.1.0");
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "a corrupt download must not install");
    assert!(err.contains("Checksum mismatch"), "{err}");
    assert!(!release.installed("weft").exists(), "nothing may be installed");
    assert!(!release.installed("ringframe").exists());
}

#[test]
fn a_release_without_a_build_for_this_platform_is_refused() {
    let release = Release::make("v0.1.0", false);
    // The stub serves only v0.1.0's names, so another tag finds nothing.
    let out = release.install("v9.9.9");
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success());
    assert!(err.contains("No build for"), "{err}");
    assert!(!release.installed("weft").exists());
}

#[test]
fn the_installer_never_builds() {
    // The whole point of the change: no toolchain on the user's machine.
    // Comments may still name the old path, so only what runs is checked.
    let text = std::fs::read_to_string(script()).expect("install.sh");
    let code: String =
        text.lines().filter(|l| !l.trim_start().starts_with('#')).collect::<Vec<_>>().join("\n");
    for builder in ["cargo ", "uv tool install", "rustc", "pip install", "npm install", "make "] {
        assert!(!code.contains(builder), "install.sh still reaches for {builder}");
    }
    assert!(code.contains("curl"), "it has to download something");
    assert!(code.contains("sha256") || code.contains("shasum"), "and verify it");
}
