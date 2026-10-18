//! A Weft opened before any installer ran (the app from Homebrew, say) sets
//! up its home from what came with it: fab7's harness and tool files and
//! RingFrame's configuration, carried in the release. What is there already
//! is left alone.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

/// A release as it is installed: `weft` and `ringframe` side by side, with
/// fab7's files in `fab7/` beside them (inside Weft.app, in its Resources).
/// The `ringframe` here says what it was asked and makes the configuration.
fn installed(dir: &Path) -> std::path::PathBuf {
    let (programs, fab7) = if cfg!(target_os = "macos") {
        let app = dir.join("Weft.app/Contents");
        (app.join("MacOS"), app.join("Resources/fab7"))
    } else {
        (dir.to_path_buf(), dir.join("fab7"))
    };
    std::fs::create_dir_all(&programs).unwrap();
    let weft = programs.join("weft");
    // A copy, not a link: Weft finds what came with it beside its own file.
    std::fs::copy(env!("CARGO_BIN_EXE_weft"), &weft).unwrap();
    let ringframe = programs.join("ringframe");
    std::fs::write(
        &ringframe,
        "#!/bin/sh\necho \"$*\" >> \"$0.asked\"\nmkdir -p \"$HOME/.fab7/rf/config\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&ringframe, std::fs::Permissions::from_mode(0o755)).unwrap();
    for (folder, file) in [
        ("weft/harnesses", "claude-code.toml"),
        ("weft/tools", "cloudflared.toml"),
        ("ringframe/config/harnesses", "claude-code.toml"),
    ] {
        std::fs::create_dir_all(fab7.join(folder)).unwrap();
        std::fs::write(fab7.join(folder).join(file), "# shipped\n").unwrap();
    }
    weft
}

/// `weft --serve` in this home, until it listens; then ended.
fn serve(weft: &Path, home: &Path, run: &Path) {
    let mut daemon = Command::new(weft)
        .arg("--serve")
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", run)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert!(line.contains("listening"), "{line}");
    let _ = daemon.kill();
    let _ = daemon.wait();
}

#[test]
fn a_first_start_sets_up_its_home_from_what_came_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let weft = installed(&dir.path().join("release"));
    let home = dir.path().join("home");
    let run = std::path::PathBuf::from(format!("/tmp/wsh-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    serve(&weft, &home, &run);
    let read = |p: &str| std::fs::read_to_string(home.join(p)).unwrap_or_default();
    assert_eq!(read(".fab7/weft/harnesses/claude-code.toml"), "# shipped\n");
    assert_eq!(read(".fab7/weft/tools/cloudflared.toml"), "# shipped\n");
    assert!(home.join(".fab7/rf/config").is_dir(), "RingFrame's configuration");
    let asked = std::fs::read_to_string(weft.with_file_name("ringframe.asked")).unwrap();
    assert!(asked.starts_with("init --global --from "), "{asked}");
    assert!(asked.trim_end().ends_with("fab7/ringframe/config"), "{asked}");

    // Set up already: nothing is replaced, and RingFrame is not asked again.
    std::fs::write(home.join(".fab7/weft/harnesses/claude-code.toml"), "# edited\n").unwrap();
    serve(&weft, &home, &run);
    assert_eq!(read(".fab7/weft/harnesses/claude-code.toml"), "# edited\n");
    let asked = std::fs::read_to_string(weft.with_file_name("ringframe.asked")).unwrap();
    assert_eq!(asked.lines().count(), 1, "{asked}");
    let _ = std::fs::remove_dir_all(&run);
}
