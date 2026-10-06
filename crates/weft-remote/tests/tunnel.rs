//! remote.md §14 test 8: `cloudflared` replaced by a script that prints its
//! box and an address, exits, and prints another: two addresses reported,
//! and the backoff between them respected (§7).

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use weft_remote::tunnel::{self, Tunnel, TunnelEvent};

/// Cloudflare Tunnel's `run`, as its adapter (the tool file) gives it.
fn quick() -> Vec<String> {
    ["tunnel", "--no-autoupdate", "--url", "http://127.0.0.1:{port}"].map(String::from).to_vec()
}

const FAKE: &str = r#"#!/bin/sh
# Where cloudflared prints its address: stderr, inside a box.
n=$(cat "$0.count" 2>/dev/null || echo 0); n=$((n + 1)); echo $n > "$0.count"
echo "args: $*" >> "$0.args"
echo "2026-10-03T00:00:00Z INF Requesting new quick Tunnel on trycloudflare.com..." >&2
echo "+--------------------------------------------------------------------------------------------+" >&2
echo "|  Your quick Tunnel has been created! Visit it at (it may take some time to be reachable):  |" >&2
echo "|  https://run-$n-quiet-words.trycloudflare.com                                              |" >&2
echo "+--------------------------------------------------------------------------------------------+" >&2
sleep 0.3
exit 1
"#;

#[test]
fn each_new_address_is_reported_and_restarts_back_off() {
    let dir = std::env::temp_dir().join(format!("weft-tunnel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("cloudflared");
    std::fs::write(&program, FAKE).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    let tunnel = Tunnel::new(program.clone(), quick(), ".trycloudflare.com".into());
    let events = tunnel.start(48123);
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while seen.iter().filter(|(e, _)| matches!(e, TunnelEvent::Address(_))).count() < 2 {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = events.recv_timeout(left).expect("two addresses within 8 s");
        seen.push((event, Instant::now()));
    }
    let addresses: Vec<_> = seen
        .iter()
        .filter_map(|(e, _)| match e {
            TunnelEvent::Address(a) => Some(a.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        addresses,
        [
            "https://run-1-quiet-words.trycloudflare.com",
            "https://run-2-quiet-words.trycloudflare.com"
        ]
    );
    let ended =
        seen.iter().find(|(e, _)| matches!(e, TunnelEvent::Ended(_))).expect("it ended between");
    let second = seen.iter().filter(|(e, _)| matches!(e, TunnelEvent::Address(_))).nth(1).unwrap();
    assert!(second.1 - ended.1 >= Duration::from_secs(2), "the first wait is 2 s");
    let args = std::fs::read_to_string(dir.join("cloudflared.args")).unwrap();
    assert_eq!(
        args.lines().next().unwrap(),
        "args: tunnel --no-autoupdate --url http://127.0.0.1:48123"
    );
    drop(tunnel);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_address_is_the_first_match_of_the_quick_tunnel_pattern() {
    let address = |line: &str| weft_remote::tunnel::address(line, ".trycloudflare.com");
    assert_eq!(
        address("|  https://seating-texas-air-guns.trycloudflare.com    |").as_deref(),
        Some("https://seating-texas-air-guns.trycloudflare.com")
    );
    assert_eq!(address("INF Requesting new quick Tunnel on trycloudflare.com..."), None);
    // cloudflared names its own API when a request fails; that is not a tunnel.
    let failed =
        r#"ERR failed to request quick Tunnel: Post "https://api.trycloudflare.com/tunnel""#;
    assert_eq!(address(failed), None);
    assert_eq!(address("https://Upper.trycloudflare.com"), None, "lower case only");
    assert_eq!(address("http://plain.trycloudflare.com"), None);
}

#[test]
fn cloudflared_is_where_the_person_said_or_on_path() {
    use weft_remote::tunnel::find;
    assert_eq!(
        find(Some("/bin/sh"), "cloudflared").as_deref(),
        Some(std::path::Path::new("/bin/sh"))
    );
    assert_eq!(find(Some("/no/such/cloudflared"), "cloudflared"), None, "said, and not there");
}

#[test]
fn dropping_the_tunnel_stops_cloudflared() {
    let dir = std::env::temp_dir().join(format!("weft-tunnel-stop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("cloudflared");
    let script = "#!/bin/sh\necho $$ > \"$0.pid\"\necho '|  https://up-a-while.trycloudflare.com  |' >&2\nexec sleep 30\n";
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let tunnel = Tunnel::new(program.clone(), quick(), ".trycloudflare.com".into());
    let events = tunnel.start(48124);
    assert!(matches!(events.recv_timeout(Duration::from_secs(5)), Ok(TunnelEvent::Address(_))));
    let pid = std::fs::read_to_string(dir.join("cloudflared.pid")).unwrap().trim().to_string();
    drop(tunnel);
    let alive = std::process::Command::new("kill").args(["-0", &pid]).status().unwrap().success();
    assert!(!alive, "cloudflared {pid} still running");
    std::fs::remove_dir_all(&dir).ok();
}

/// ADR-0024: another adapter, with its own `run` and `address`, is started
/// and read by its own; nothing of Cloudflare's applies to it.
#[test]
fn another_adapter_is_started_and_read_by_its_own_file() {
    let dir = std::env::temp_dir().join(format!("weft-tunnel-other-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("othertunnel");
    std::fs::write(
        &program,
        "#!/bin/sh\necho \"args: $*\" >> \"$0.args\"\necho 'see https://x.trycloudflare.com' >&2\necho 'ready at https://calm-fox.tunnels.example.net/' >&2\nexec sleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = ["expose", "{port}", "--quiet"].map(String::from).to_vec();
    let tunnel = Tunnel::new(program.clone(), run, ".tunnels.example.net".into());
    let events = tunnel.start(48124);
    let first = events.recv_timeout(Duration::from_secs(8)).expect("an address");
    assert_eq!(first, TunnelEvent::Address("https://calm-fox.tunnels.example.net".into()));
    let args = std::fs::read_to_string(dir.join("othertunnel.args")).unwrap();
    assert_eq!(args.lines().next().unwrap(), "args: expose 48124 --quiet");
    drop(tunnel);
    std::fs::remove_dir_all(&dir).ok();
}

/// A weft remote killed by a signal never drops its tunnel: the program it
/// started is left running. The next one stops it from the pid it kept.
#[test]
fn a_tunnel_left_by_a_killed_weft_remote_is_stopped_by_the_next() {
    let dir = std::env::temp_dir().join(format!("weft-tunnel-left-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("cloudflared");
    // Not exec: the process keeps the program's name, as cloudflared's does.
    let script = "#!/bin/sh\necho '|  https://left-over.trycloudflare.com  |' >&2\nsleep 30\n";
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let kept = dir.join("tunnel.pid");
    let left = Tunnel::new(program.clone(), quick(), ".trycloudflare.com".into())
        .keeping_pid(kept.clone());
    let events = left.start(48125);
    assert!(matches!(events.recv_timeout(Duration::from_secs(5)), Ok(TunnelEvent::Address(_))));
    let pid = std::fs::read_to_string(&kept).unwrap().trim().to_string();
    // Killed: no drop, so nothing stops it.
    std::mem::forget(left);
    let alive = |pid: &str| {
        std::process::Command::new("kill").args(["-0", pid]).status().unwrap().success()
    };
    assert!(alive(&pid));
    tunnel::stop_left(&kept, &program);
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(&pid) {
        assert!(Instant::now() < deadline, "the left tunnel {pid} still runs");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!kept.exists());
    // A pid that is not this program's is never touched.
    std::fs::write(&kept, std::process::id().to_string()).unwrap();
    tunnel::stop_left(&kept, &program);
    std::fs::remove_dir_all(&dir).ok();
}
