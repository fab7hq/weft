//! `weft stop` stops the daemon whatever protocol it speaks. An upgrade
//! installs a `weft` whose protocol is newer than the daemon still running;
//! `bin/reinstall-local` stops that daemon before starting the new one, and
//! if the stop needed a hello the old daemon would refuse it ("this daemon
//! speaks protocol 3, not 4") and keep running beside the new one.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};

use weft::protocol;
use weft::server;

#[test]
fn stop_reaches_a_daemon_of_another_protocol() {
    let socket = protocol::private_socket("weft-old-daemon");
    let listener = UnixListener::bind(&socket).unwrap();
    // An older daemon: it refuses any hello and stops on `shutdown`.
    let old = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut heard = Vec::new();
        for line in BufReader::new(stream).lines() {
            let line = line.unwrap();
            let m: serde_json::Value = serde_json::from_str(&line).unwrap();
            let method = m["method"].as_str().unwrap_or_default().to_string();
            heard.push(method.clone());
            let id = m["id"].clone();
            if method == "hello" {
                let no = serde_json::json!({"id": id, "error": {"code": "protocol",
                    "message": "this daemon speaks protocol 3, not 4"}});
                writeln!(writer, "{no}").unwrap();
            } else if method == "shutdown" {
                let yes = serde_json::json!({"id": id, "result": {}});
                writeln!(writer, "{yes}").unwrap();
                break;
            }
        }
        heard
    });
    protocol::stop(&socket).expect("stopped");
    assert_eq!(old.join().unwrap(), ["shutdown"], "no hello it would refuse");
}

#[test]
fn stop_ends_a_running_daemon() {
    let socket = protocol::private_socket("weft-stop");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !protocol::is_live(&socket) {
        assert!(Instant::now() < deadline, "the daemon never came up");
        std::thread::sleep(Duration::from_millis(20));
    }
    protocol::stop(&socket).expect("stopped");
    let deadline = Instant::now() + Duration::from_secs(5);
    while protocol::is_live(&socket) {
        assert!(Instant::now() < deadline, "the daemon is still running");
        std::thread::sleep(Duration::from_millis(20));
    }
}
