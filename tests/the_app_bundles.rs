//! `bin/bundle` makes Weft.app from built binaries: the desktop app as the
//! bundle's program, `weft` and `ringframe` beside it, the mark as its icon,
//! signed so `codesign --verify` passes. The Linux desktop entry starts the
//! app by name.

use std::path::Path;
use std::process::Command;

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
        .output()
        .expect("bin/bundle runs");
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let app = out.join("Weft.app/Contents");
    for name in ["weft-desktop", "weft", "ringframe"] {
        assert!(app.join("MacOS").join(name).is_file(), "{name}");
    }
    assert!(app.join("Resources/weft.icns").is_file());
    let plist = std::fs::read_to_string(app.join("Info.plist")).unwrap();
    for held in [
        "<key>CFBundleExecutable</key>\n    <string>weft-desktop</string>",
        "<key>CFBundleIconFile</key>\n    <string>weft</string>",
        &format!("<key>CFBundleShortVersionString</key>\n    <string>{}</string>", env!("CARGO_PKG_VERSION")),
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
    let entry =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/linux/weft.desktop"))
            .unwrap();
    for line in ["[Desktop Entry]", "Type=Application", "Name=Weft", "Exec=weft-desktop %f", "Icon=weft"] {
        assert!(entry.lines().any(|l| l == line), "{line}\n{entry}");
    }
}
