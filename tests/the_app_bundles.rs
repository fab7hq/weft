//! `bin/bundle` makes Weft.app from built binaries: the desktop app as the
//! bundle's program, `weft` and `ringframe` beside it, the mark as its icon,
//! signed so `codesign --verify` passes. The Linux desktop entry starts the
//! app by name.

use std::path::Path;
use std::process::Command;

/// A fab7 checkout with one file in each part a release carries.
fn fab7(dir: &Path) -> std::path::PathBuf {
    let fab7 = dir.join("fab7");
    for (part, file) in [
        ("ringframe", "bundle.toml"),
        ("ringframe/config/harnesses", "codex.toml"),
        ("weft/harnesses", "codex.toml"),
        ("weft/tools", "cloudflared.toml"),
    ] {
        let at = fab7.join("products").join(part);
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(at.join(file), "").unwrap();
    }
    fab7
}

/// What came with the app, as a first start reads it.
const CARRIED: [&str; 4] = [
    "fab7/ringframe/bundle.toml",
    "fab7/ringframe/config/harnesses/codex.toml",
    "fab7/weft/harnesses/codex.toml",
    "fab7/weft/tools/cloudflared.toml",
];

fn stub(dir: &Path, name: &str) {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\necho {name}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
#[cfg(target_os = "macos")]
fn the_bundle_holds_the_app_its_tools_and_icon_and_is_signed() {
    let tmp = tempfile::tempdir().unwrap();
    let built = tmp.path().join("built");
    std::fs::create_dir_all(&built).unwrap();
    for name in ["weft-desktop", "weft", "ringframe"] {
        stub(&built, name);
    }
    let out = tmp.path().join("out");
    let made = Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/bundle"))
        .arg(&built)
        .arg(&out)
        .arg(fab7(tmp.path()))
        .output()
        .expect("bin/bundle runs");
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let app = out.join("Weft.app/Contents");
    for name in ["weft-desktop", "weft", "ringframe"] {
        assert!(app.join("MacOS").join(name).is_file(), "{name}");
    }
    assert!(app.join("Resources/weft.icns").is_file());
    for file in CARRIED {
        assert!(app.join("Resources").join(file).is_file(), "{file}");
    }
    let plist = std::fs::read_to_string(app.join("Info.plist")).unwrap();
    for held in [
        "<key>CFBundleExecutable</key>\n    <string>weft-desktop</string>",
        "<key>CFBundleIconFile</key>\n    <string>weft</string>",
        &format!(
            "<key>CFBundleShortVersionString</key>\n    <string>{}</string>",
            env!("CARGO_PKG_VERSION")
        ),
    ] {
        assert!(plist.contains(held), "{held}\n{plist}");
    }
    let verified = Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(out.join("Weft.app"))
        .output()
        .unwrap();
    assert!(verified.status.success(), "{}", String::from_utf8_lossy(&verified.stderr));
}

#[test]
fn the_linux_entry_starts_the_app_with_its_icon() {
    let entry = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/linux/weft.desktop"),
    )
    .unwrap();
    for line in [
        "[Desktop Entry]",
        "Type=Application",
        "Name=Weft",
        "Exec=weft-desktop %f",
        "Icon=weft",
        "StartupWMClass=weft-desktop",
    ] {
        assert!(entry.lines().any(|l| l == line), "{line}\n{entry}");
    }
}

/// `bin/pack` makes a target's release archive as install.sh reads it: on
/// macOS Weft.app alone; on Linux the three programs, the entry and the icon.
#[test]
fn a_release_archive_holds_the_app() {
    let tmp = tempfile::tempdir().unwrap();
    let built = tmp.path().join("built");
    std::fs::create_dir_all(&built).unwrap();
    for name in ["weft-desktop", "weft", "ringframe"] {
        stub(&built, name);
    }
    let target = if cfg!(target_os = "macos") {
        "aarch64-apple-darwin"
    } else {
        "x86_64-unknown-linux-musl"
    };
    let made = Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/pack"))
        .args(["v0.9.0", target])
        .arg(&built)
        .arg(&built)
        .arg(fab7(tmp.path()))
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let archive = tmp.path().join(format!("fab7-v0.9.0-{target}.tar.gz"));
    assert!(tmp.path().join(format!("fab7-v0.9.0-{target}.tar.gz.sha256")).is_file());
    let listed = Command::new("tar").arg("-tzf").arg(&archive).output().unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout);
    let held: Vec<&str> = listed.lines().filter(|l| !l.ends_with('/')).collect();
    if cfg!(target_os = "macos") {
        for f in [
            "Weft.app/Contents/MacOS/weft-desktop",
            "Weft.app/Contents/MacOS/weft",
            "Weft.app/Contents/MacOS/ringframe",
            "Weft.app/Contents/Info.plist",
        ] {
            assert!(held.contains(&f), "{f}: {held:?}");
        }
        for f in CARRIED {
            assert!(held.contains(&format!("Weft.app/Contents/Resources/{f}").as_str()), "{f}");
        }
    } else {
        let mut held = held.clone();
        held.sort();
        let mut want = vec!["ringframe", "weft", "weft-desktop", "weft.desktop", "weft.svg"];
        want.extend(CARRIED);
        want.sort();
        assert_eq!(held, want);
    }
}

/// `bin/cask` writes the tap's cask for a release from its two macOS
/// checksums: Weft.app, `weft` and `ringframe` on PATH, the quarantine taken
/// away; Ruby reads it. A tag or checksum that is not one writes nothing.
#[test]
#[cfg(target_os = "macos")]
fn the_cask_installs_the_release_archives() {
    let tmp = tempfile::tempdir().unwrap();
    let cask = |args: &[&str]| {
        Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/cask"))
            .args(args)
            .current_dir(tmp.path())
            .output()
            .unwrap()
    };
    let (arm, intel) = ("a".repeat(64), "b".repeat(64));
    let made = cask(&["v0.9.0", &arm, &intel]);
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let rb = std::fs::read_to_string(tmp.path().join("Casks/weft.rb")).unwrap();
    for held in [
        "cask \"weft\" do",
        "version \"0.9.0\"",
        &format!("sha256 arm:   \"{arm}\","),
        &format!("intel: \"{intel}\""),
        "fab7-v#{version}-#{arch}-apple-darwin.tar.gz",
        "app \"Weft.app\"",
        "binary \"#{appdir}/Weft.app/Contents/MacOS/weft\"",
        "binary \"#{appdir}/Weft.app/Contents/MacOS/ringframe\"",
        "\"-dr\", \"com.apple.quarantine\", \"#{appdir}/Weft.app\"",
        "zap trash: \"~/.fab7/weft\"",
    ] {
        assert!(rb.contains(held), "{held}\n{rb}");
    }
    std::fs::remove_file(tmp.path().join("Casks/weft.rb")).unwrap();
    assert!(!cask(&["0.9.0", &arm, &intel]).status.success(), "not a release tag");
    assert!(!cask(&["v0.9.0", "abc", &intel]).status.success(), "not a checksum");
    let two_lines = format!("{arm}\n\"; system \"true");
    assert!(!cask(&["v0.9.0", &two_lines, &intel]).status.success(), "checked whole");
    assert!(!cask(&["v0.9.0\"; system \"true", &arm, &intel]).status.success(), "a tag, whole");
    assert!(!cask(&["v0.9.0\n1", &arm, &intel]).status.success(), "a tag on one line");
    assert!(!tmp.path().join("Casks/weft.rb").exists());
}
