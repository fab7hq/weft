//! `install.sh` downloads and verifies; it never builds.
//!
//! This stubs `curl`, the only thing the script reaches the network with, and
//! serves a release made on the spot. Everything else — the platform
//! detection, the checksum, the unpacking, the install — is the real script.
//!
//! What a release holds, per system: on macOS Weft.app, with `weft` and
//! `ringframe` inside it and fab7's files in its Resources; on Linux the
//! three programs, the desktop entry, the icon and fab7's files.

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
        let programs = if cfg!(target_os = "macos") {
            staged.join("Weft.app/Contents/MacOS")
        } else {
            staged.clone()
        };
        std::fs::create_dir_all(&programs).unwrap();
        for name in ["weft", "ringframe", "weft-desktop"] {
            let p = programs.join(name);
            // `ringframe init --global` records the fab7 tag it installed, as
            // the real one does.
            let init = format!(
                "if [ \"${{1:-}}\" = init ]; then mkdir -p \"$HOME/.fab7/rf/config\" && \
                 echo {tag} > \"$HOME/.fab7/rf/config/.revision\"; fi\n"
            );
            std::fs::write(&p, format!("#!/bin/sh\n{init}echo \"{name} {tag}\"\n")).unwrap();
            make_executable(&p);
        }
        // fab7's files, as the release carries them: Weft's harness and
        // tool files, and RingFrame's configuration.
        let fab7 = if cfg!(target_os = "macos") {
            staged.join("Weft.app/Contents/Resources/fab7")
        } else {
            staged.join("fab7")
        };
        for (dir, file, text) in [
            ("weft/harnesses", "zed-agent.toml", "title = \"Zed Agent\"\n"),
            ("weft/tools", "cloudflared.toml", "program = \"cloudflared\"\n"),
            ("ringframe/config/harnesses", "zed-agent.toml", ""),
        ] {
            std::fs::create_dir_all(fab7.join(dir)).unwrap();
            std::fs::write(fab7.join(dir).join(file), text).unwrap();
        }
        let packed: &[&str] = if cfg!(target_os = "macos") {
            std::fs::write(staged.join("Weft.app/Contents/Info.plist"), plist("com.getfab7.weft"))
                .unwrap();
            &["Weft.app"]
        } else {
            let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
            std::fs::copy(assets.join("linux/weft.desktop"), staged.join("weft.desktop")).unwrap();
            std::fs::copy(assets.join("weft-mark.svg"), staged.join("weft.svg")).unwrap();
            &["weft", "ringframe", "weft-desktop", "weft.desktop", "weft.svg", "fab7"]
        };
        let mut tar = vec!["tar".to_string(), "-czf".into()];
        tar.push(serve.join(&archive).to_string_lossy().into_owned());
        tar.push("-C".into());
        tar.push(staged.to_string_lossy().into_owned());
        tar.extend(packed.iter().map(|p| p.to_string()));
        run(&tar.iter().map(String::as_str).collect::<Vec<_>>());

        let digest = sha256_of(&serve.join(&archive));
        let written = if corrupt_checksum { "0".repeat(64) } else { digest };
        std::fs::write(serve.join(format!("{archive}.sha256")), format!("{written}  {archive}\n"))
            .unwrap();
        // Where GitHub's latest release page sends a browser.
        std::fs::write(
            serve.join("releases-latest"),
            format!("https://github.com/fab7hq/weft/releases/tag/{tag}"),
        )
        .unwrap();

        // A `curl -fsSL [-o OUT] URL` that reads from `serve/` by the last
        // path segment, and fails on anything it was not given. GitHub's API
        // refuses it, as it refuses an address many people share; `-w` with
        // the latest release page prints where it leads.
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let curl = bin.join("curl");
        let stub = r#"#!/bin/sh
# A `curl -fsSL [-o OUT] URL` that serves SERVE_DIR by the URL's last segment.
set -eu
serve='SERVE_DIR'
out=''
url=''
written=''
while [ $# -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift 2 ;;
        -w) written=$2; shift 2 ;;
        -*) shift ;;
        *) url=$1; shift ;;
    esac
done
case "$url" in
    *api.github.com*) exit 22 ;;
    *releases/latest) [ "$written" = '%{url_effective}' ] || exit 22; cat "$serve/releases-latest"; exit 0 ;;
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
        self.install_into(tag, &self.dir.path().join("home"), &self.dir.path().join("target-bin"))
    }

    /// Install `tag` from this release into another home and bin folder.
    fn install_into(&self, tag: &str, home: &Path, bin_dir: &Path) -> std::process::Output {
        self.command(home, bin_dir).env("WEFT_VERSION", tag).output().expect("sh")
    }

    /// Install the latest release, as the documented command does.
    fn install_latest(&self) -> std::process::Output {
        let (home, bin_dir) = (self.dir.path().join("home"), self.dir.path().join("target-bin"));
        self.command(&home, &bin_dir).output().expect("sh")
    }

    fn command(&self, home: &Path, bin_dir: &Path) -> Command {
        std::fs::create_dir_all(home).unwrap();
        let mut command = Command::new("sh");
        command
            .arg(script())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.dir.path().join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("WEFT_BIN_DIR", bin_dir)
            .env("WEFT_APPS_DIR", home.join("Applications"))
            .env("WEFT_SYSTEM_PACKAGES", "0")
            .env("HOME", home);
        command
    }

    fn installed(&self, name: &str) -> PathBuf {
        self.dir.path().join("target-bin").join(name)
    }
}

/// An app's Info.plist, naming the app `id`.
fn plist(id: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
         <key>CFBundleIdentifier</key><string>{id}</string></dict></plist>\n"
    )
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh")
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") { "apple-darwin" } else { "unknown-linux-gnu" }
}

/// What install.sh says on a system that `uname -s` and `uname -m` name,
/// whose `ldd --version` prints `ldd`.
fn install_on(system: &str, machine: &str, ldd: &str) -> String {
    let release = Release::make("v0.1.0", false);
    let bin = release.dir.path().join("bin");
    let uname =
        format!("#!/bin/sh\ncase \"$1\" in -s) echo {system} ;; -m) echo {machine} ;; esac\n");
    std::fs::write(bin.join("uname"), uname).unwrap();
    std::fs::write(bin.join("ldd"), format!("#!/bin/sh\necho '{ldd}'\n")).unwrap();
    make_executable(&bin.join("uname"));
    make_executable(&bin.join("ldd"));
    let out = release.install("v0.1.0");
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
fn linux_takes_the_gnu_build_and_an_intel_mac_a_musl_or_an_old_glibc_system_is_told_it_has_none() {
    for machine in ["x86_64", "aarch64"] {
        let said = install_on("Linux", machine, "ldd (Ubuntu GLIBC 2.39-0ubuntu8.6) 2.39");
        assert!(said.contains(&format!("for {machine}-unknown-linux-gnu")), "{said}");
    }
    let said = install_on("Linux", "x86_64", "musl libc (x86_64)");
    assert!(said.contains("Weft has no build for a musl system"), "{said}");
    assert!(!said.contains("Downloading"), "{said}");
    let said = install_on("Linux", "aarch64", "ldd (Debian GLIBC 2.36-9+deb12u14) 2.36");
    assert!(said.contains("Weft needs glibc 2.38 or later; this system has 2.36"), "{said}");
    assert!(!said.contains("Downloading"), "{said}");
    let said = install_on("Darwin", "x86_64", "");
    assert!(said.contains("Weft has no build for an Intel Mac"), "{said}");
    assert!(!said.contains("Downloading"), "{said}");
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
fn the_latest_release_is_found_without_githubs_api() {
    let release = Release::make("v0.1.0", false);
    let out = release.install_latest();
    let text =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("Downloading fab7hq/weft v0.1.0"), "{text}");
    assert!(release.installed("weft").exists(), "{text}");
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
        assert!(p.is_symlink(), "{name} links into the installed Weft");
        let shown = Command::new(&p).output().expect("the installed binary runs");
        assert_eq!(
            String::from_utf8_lossy(&shown.stdout).trim(),
            format!("{name} v0.1.0"),
            "{name} is not what the release contained"
        );
    }
    assert!(text.contains("Downloading fab7hq/weft v0.1.0"), "{text}");
    let home = release.dir.path().join("home");
    if cfg!(target_os = "macos") {
        // The app itself is in Applications, as Homebrew puts it, where
        // Finder and Spotlight list it; the tools on PATH are its own.
        let app = home.join("Applications/Weft.app");
        assert!(!app.is_symlink(), "{text}");
        assert!(app.join("Contents/MacOS/weft-desktop").is_file(), "{text}");
        for name in ["weft", "ringframe"] {
            assert_eq!(
                std::fs::read_link(release.installed(name)).unwrap(),
                app.join("Contents/MacOS").join(name)
            );
        }
    } else {
        // Weft is in its version's folder, and `weft` is a link to it.
        let version = home.join(".fab7/weft/versions/0.1.0");
        assert!(version.join("weft").is_file(), "{text}");
        assert_eq!(std::fs::read_link(release.installed("weft")).unwrap(), version.join("weft"));
        assert_eq!(
            std::fs::read_link(release.installed("weft-desktop")).unwrap(),
            version.join("weft-desktop")
        );
        let entry =
            std::fs::read_to_string(home.join(".local/share/applications/weft.desktop")).unwrap();
        let exec = format!("Exec={} %f", release.installed("weft-desktop").display());
        assert!(entry.lines().any(|l| l == exec), "{exec}\n{entry}");
        assert!(home.join(".local/share/icons/hicolor/scalable/apps/weft.svg").is_file());
    }
}

/// On Linux a newer version goes beside the older, whose file a running
/// Weft still uses; on macOS it takes the older app's place. `weft` then
/// runs the newer.
#[test]
fn a_newer_version_takes_over_from_the_older() {
    let old = Release::make("v0.1.0", false);
    let new = Release::make("v0.2.0", false);
    let (home, bin) = (old.dir.path().join("home"), old.dir.path().join("target-bin"));
    assert!(old.install("v0.1.0").status.success() || bin.join("weft").exists());
    let out = new.install_into("v0.2.0", &home, &bin);
    let text = String::from_utf8_lossy(&out.stderr);
    let shown = Command::new(bin.join("weft")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&shown.stdout).trim(), "weft v0.2.0", "{text}");
    if cfg!(target_os = "macos") {
        let apps: Vec<_> =
            std::fs::read_dir(home.join("Applications")).unwrap().flatten().collect();
        assert_eq!(apps.len(), 1, "one app: the newer one");
    } else {
        let versions = home.join(".fab7/weft/versions");
        let before = std::fs::read(versions.join("0.1.0/weft")).unwrap();
        assert!(String::from_utf8_lossy(&before).contains("v0.1.0"), "untouched: {text}");
        assert_eq!(std::fs::read_link(bin.join("weft")).unwrap(), versions.join("0.2.0/weft"));
        assert_eq!(
            std::fs::read_link(bin.join("weft-desktop")).unwrap(),
            versions.join("0.2.0/weft-desktop")
        );
    }
}

/// Weft's harness files come with the release, and replace whatever was
/// there: they are never edited.
#[test]
fn the_harness_files_come_with_the_release_and_replace_the_old() {
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

/// The tool files come the same way, and replace the old the same
/// way.
#[test]
fn the_tool_files_are_installed_beside_the_harness_files() {
    let release = Release::make("v0.1.0", false);
    let installed = release.dir.path().join("home/.fab7/weft/tools");
    std::fs::create_dir_all(&installed).unwrap();
    std::fs::write(installed.join("gone.toml"), "program = \"gone\"\n").unwrap();
    let out = release.install("v0.1.0");
    let text =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(installed.join("cloudflared.toml").is_file(), "{text}");
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

/// An app in Applications that Weft did not install is left alone, and
/// nothing is switched: the install stops before it starts.
#[test]
#[cfg(target_os = "macos")]
fn a_weft_app_weft_did_not_install_stops_the_install_first() {
    let release = Release::make("v0.1.0", false);
    let home = release.dir.path().join("home");
    std::fs::create_dir_all(home.join("Applications/Weft.app")).unwrap();
    let out = release.install("v0.1.0");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not one Weft installed"));
    assert!(!release.installed("weft").exists(), "nothing linked");
    assert!(!home.join(".fab7/weft/versions/0.1.0").exists(), "nothing installed");
}

/// An earlier Weft in Applications, the app or the link an older installer
/// made, is replaced by this one.
#[test]
#[cfg(target_os = "macos")]
fn an_earlier_weft_app_is_replaced() {
    for earlier_is_a_link in [false, true] {
        let release = Release::make("v0.1.0", false);
        let home = release.dir.path().join("home");
        let app = home.join("Applications/Weft.app");
        if earlier_is_a_link {
            let old = home.join(".fab7/weft/versions/0.0.9/Weft.app");
            std::fs::create_dir_all(&old).unwrap();
            std::fs::create_dir_all(home.join("Applications")).unwrap();
            std::os::unix::fs::symlink(&old, &app).unwrap();
        } else {
            std::fs::create_dir_all(app.join("Contents")).unwrap();
            std::fs::write(app.join("Contents/Info.plist"), plist("com.getfab7.weft")).unwrap();
        }
        let out = release.install("v0.1.0");
        let text = String::from_utf8_lossy(&out.stderr);
        assert!(!app.is_symlink(), "{text}");
        assert!(app.join("Contents/MacOS/weft-desktop").is_file(), "{text}");
    }
}

/// On Linux, what Weft needs of the system and does not find comes from its
/// package manager: git, and the libraries the app draws with.
#[test]
fn linux_gets_what_weft_needs_from_its_package_manager() {
    let release = Release::make("v0.1.0", false);
    let bin = release.dir.path().join("bin");
    let asked = release.dir.path().join("asked");
    let stubs = [
        ("uname", "case \"$1\" in -s) echo Linux ;; -m) echo x86_64 ;; esac".to_string()),
        ("ldd", "echo 'ldd (Ubuntu GLIBC 2.39-0ubuntu8.6) 2.39'".to_string()),
        // A system with none of the libraries, as a server or a container.
        ("ldconfig", "echo '0 libs found in cache'".to_string()),
        ("id", "echo 0".to_string()),
        ("apt-get", format!("echo \"$*\" >> '{}'", asked.display())),
        ("env", "shift; exec \"$@\"".to_string()),
    ];
    for (name, body) in stubs {
        std::fs::write(bin.join(name), format!("#!/bin/sh\n{body}\n")).unwrap();
        make_executable(&bin.join(name));
    }
    let home = release.dir.path().join("home");
    let out = release
        .command(&home, &release.dir.path().join("target-bin"))
        .env("WEFT_VERSION", "v0.1.0")
        .env_remove("WEFT_SYSTEM_PACKAGES")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    let asked = std::fs::read_to_string(&asked).unwrap_or_default();
    assert!(said.contains("Weft needs") && said.contains("libxkbcommon-x11.so.0"), "{said}");
    assert!(asked.lines().any(|l| l == "update -q"), "{asked}");
    let install = asked.lines().find(|l| l.starts_with("install -y")).unwrap_or_default();
    for package in ["libxcb1", "libxkbcommon-x11-0", "libvulkan1", "mesa-vulkan-drivers"] {
        assert!(install.split(' ').any(|p| p == package), "{package}: {install}");
    }
}
