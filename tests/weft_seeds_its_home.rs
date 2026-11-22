//! A Weft opened before any installer ran (the app from Homebrew, say) sets
//! up its home from the newest fab7 release it runs with. With no network and
//! nothing installed it uses the copy that came with it, recorded as that; a
//! home it runs with is left alone, and a home it cannot run with is never
//! overwritten by that copy.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use weft::protocol::{self, Call, Line, PROTOCOL};

/// A release as it is installed: `weft`, with fab7's files in `fab7/` beside
/// it (inside Weft.app, in its Resources), recording the fab7 release they
/// came from.
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
    let config =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/ringframe/tests/fixtures/config");
    copy(&config, &fab7.join("ringframe/config"));
    for (folder, file) in
        [("weft/harnesses", "claude-code.toml"), ("weft/tools", "cloudflared.toml")]
    {
        std::fs::create_dir_all(fab7.join(folder)).unwrap();
        std::fs::write(fab7.join(folder).join(file), "# shipped\n").unwrap();
    }
    std::fs::write(fab7.join(".revision"), format!("v{}\n", weftd::fab7::NEEDS)).unwrap();
    weft
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `weft --serve` in this home, with fab7's releases out of reach, until it
/// answers a first call, which it does once its home is prepared; then ended.
fn serve(weft: &Path, home: &Path, run: &Path) {
    let offline = home.parent().unwrap().join("offline.gitconfig");
    std::fs::write(&offline, "[url \"file:///nowhere/\"]\n\tinsteadOf = https://github.com/\n")
        .unwrap();
    let mut daemon = Command::new(weft)
        .arg("--serve")
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", run)
        .env("GIT_CONFIG_GLOBAL", &offline)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert!(line.contains("listening"), "{line}");
    let mut stream = UnixStream::connect(run.join("weft").join("weft.sock")).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let hello =
        Line::Call { id: 1, call: Call::Hello { client: "test".into(), protocol: PROTOCOL } };
    protocol::send(&mut stream, &hello.encode()).unwrap();
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).unwrap();
    assert!(!answer.is_empty(), "it answers once prepared");
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
    let bundled = format!("bundled v{}\n", weftd::fab7::NEEDS);
    assert_eq!(read(".fab7/weft/harnesses/claude-code.toml"), "# shipped\n");
    assert_eq!(read(".fab7/weft/tools/cloudflared.toml"), "# shipped\n");
    for dir in [".fab7/rf/config", ".fab7/weft/harnesses", ".fab7/weft/tools"] {
        assert_eq!(read(&format!("{dir}/.revision")), bundled, "{dir}");
    }

    // A home this Weft runs with: nothing is replaced.
    std::fs::write(home.join(".fab7/weft/harnesses/claude-code.toml"), "# edited\n").unwrap();
    serve(&weft, &home, &run);
    assert_eq!(read(".fab7/weft/harnesses/claude-code.toml"), "# edited\n");

    // A home of an older fab7, with the network out of reach: it waits for the
    // network, and the copy that came with this Weft never overwrites it.
    for dir in [".fab7/rf/config", ".fab7/weft/harnesses", ".fab7/weft/tools"] {
        std::fs::write(home.join(dir).join(".revision"), "v0.7.1\n").unwrap();
    }
    serve(&weft, &home, &run);
    assert_eq!(read(".fab7/weft/harnesses/claude-code.toml"), "# edited\n");
    assert_eq!(read(".fab7/rf/config/.revision"), "v0.7.1\n");
    let _ = std::fs::remove_dir_all(&run);
}
