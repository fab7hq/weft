//! The harness process a hook ran under: the first ancestor of
//! the hook whose name is the harness's program, and when it started. Read
//! from the kernel, never by starting `ps` in a hook: on Linux from
//! `/proc/<pid>/stat` and `/proc/<pid>/cmdline`, on macOS from `proc_pidinfo`
//! and the `KERN_PROCARGS2` sysctl.

/// A process, as a hook found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    /// When it started, in the kernel's own units: a pid with another start
    /// time is another process.
    pub start: String,
}

/// What one process says of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// Its `comm`.
    pub name: String,
    /// The base name of its `argv[0]`, when it can be read.
    pub argv0: Option<String>,
    /// The words after `argv[0]`, when they can be read.
    pub args: Vec<String>,
    pub parent: u32,
    pub start: String,
    /// It has exited and is not yet reaped: gone, for anyone asking.
    pub zombie: bool,
}

/// Whether the process a session's receipts name still runs: that pid, with
/// that start time, and not a zombie.
pub fn alive(p: &weft_core::sessions::Process) -> bool {
    info(p.pid).is_some_and(|me| me.start == p.start && !me.zombie)
}

/// The first process at or above `from` whose name, or else whose
/// `argv[0]`'s base name, is `program`'s base name; none when that process
/// is the harness's shared server, whose arguments hold one of `server`
/// (ADR-0035 D4): it holds every session, so it is no session's, and the
/// program above it only started it.
fn walk(
    from: u32,
    program: &str,
    server: &[String],
    info: &dyn Fn(u32) -> Option<Info>,
) -> Option<Process> {
    let want = base(program);
    let mut pid = from;
    // A chain of processes is short; this only stops a cycle from spinning.
    for _ in 0..64 {
        if pid == 0 {
            return None;
        }
        let me = info(pid)?;
        if me.name == want || me.argv0.as_deref() == Some(want) {
            if me.args.iter().any(|a| server.contains(a)) {
                return None;
            }
            return Some(Process { pid, start: me.start });
        }
        if me.parent == pid {
            return None;
        }
        pid = me.parent;
    }
    None
}

/// The harness process this one runs under, unless it is the harness's
/// shared server.
pub fn harness(program: &str, server: &[String]) -> Option<Process> {
    walk(std::os::unix::process::parent_id(), program, server, &info)
}

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `/proc/<pid>/stat`: the name is between the first `(` and the last `)`,
/// since a name may hold either; after it, the state is field 3, the parent
/// field 4 and the start time, in clock ticks since boot, field 22.
#[cfg(target_os = "linux")]
pub fn info(pid: u32) -> Option<Info> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (open, close) = (stat.find('(')?, stat.rfind(')')?);
    let name = stat.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    let zombie = rest.first() == Some(&"Z");
    let parent = rest.get(1)?.parse().ok()?;
    let start = rest.get(19)?.to_string();
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let mut words = cmdline.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned());
    let argv0 = words.next().filter(|a| !a.is_empty()).map(|a| base(&a).to_string());
    let mut args: Vec<String> = words.collect();
    // `cmdline` ends with a NUL, which leaves one empty word after the last.
    if args.last().is_some_and(String::is_empty) {
        args.pop();
    }
    Some(Info { name, argv0, args, parent, start, zombie })
}

/// `proc_pidinfo(PROC_PIDTBSDINFO)`: the name (`p_comm`), the parent and the
/// start time, seconds and microseconds; its argv from `KERN_PROCARGS2`.
#[cfg(target_os = "macos")]
pub fn info(pid: u32) -> Option<Info> {
    let mut bsd: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `bsd` is a zeroed `proc_bsdinfo` of exactly `size` bytes.
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut bsd as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if got != size {
        return None;
    }
    let comm: Vec<u8> = bsd.pbi_comm.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    let mut words = argv(pid).unwrap_or_default().into_iter();
    Some(Info {
        name: String::from_utf8_lossy(&comm).into_owned(),
        argv0: words.next().map(|a| base(&a).to_string()),
        args: words.collect(),
        parent: bsd.pbi_ppid,
        start: format!("{}.{:06}", bsd.pbi_start_tvsec, bsd.pbi_start_tvusec),
        // `SZOMB` in <sys/proc.h>.
        zombie: bsd.pbi_status == 5,
    })
}

/// `KERN_PROCARGS2` holds `argc`, the executable's path, padding, then argv.
#[cfg(target_os = "macos")]
fn argv(pid: u32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut buf = vec![0u8; 64 * 1024];
    let mut len = buf.len();
    // SAFETY: `buf` is `len` bytes, and the kernel writes at most that many.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok != 0 || len < 4 {
        return None;
    }
    let argc = i32::from_ne_bytes(buf[..4].try_into().ok()?) as usize;
    let rest = &buf[4..len];
    let path_end = rest.iter().position(|b| *b == 0)?;
    let args = &rest[path_end..];
    let start = args.iter().position(|b| *b != 0)?;
    let words = args[start..].split(|b| *b == 0).take(argc);
    Some(words.map(|a| String::from_utf8_lossy(a).into_owned()).collect())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn info(_pid: u32) -> Option<Info> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_reads_its_own_name_parent_and_start() {
        let me = info(std::process::id()).expect("this process");
        assert_eq!(me.parent, std::os::unix::process::parent_id());
        assert!(!me.name.is_empty() && !me.start.is_empty(), "{me:?}");
        assert_eq!(info(std::process::id()).unwrap().start, me.start, "the same each time");
    }

    #[test]
    fn the_harness_is_the_first_ancestor_running_its_program() {
        let me = std::process::id();
        let parent = info(me).unwrap().parent;
        let up = info(parent).expect("the parent");
        let found = walk(me, &up.name, &[], &info).expect("the parent, by name");
        assert_eq!(found, Process { pid: parent, start: up.start.clone() });
        // A path names its program by its base name.
        let path = format!("/somewhere/bin/{}", up.name);
        assert_eq!(walk(me, &path, &[], &info).map(|p| p.pid), Some(parent));
        assert_eq!(walk(me, "no-such-program-weft", &[], &info), None);
    }

    #[test]
    fn a_process_reads_its_own_arguments() {
        let me = info(std::process::id()).expect("this process");
        assert_eq!(me.args, std::env::args().skip(1).collect::<Vec<_>>());
    }

    /// A chain of processes, child first: (pid, name, args).
    fn chain(links: &[(u32, &str, &[&str])]) -> impl Fn(u32) -> Option<Info> {
        let links: Vec<(u32, Info)> = links
            .iter()
            .enumerate()
            .map(|(i, (pid, name, args))| {
                let parent = links.get(i + 1).map_or(1, |l| l.0);
                let args = args.iter().map(|a| a.to_string()).collect();
                (
                    *pid,
                    Info {
                        name: name.to_string(),
                        argv0: Some(name.to_string()),
                        args,
                        parent,
                        start: format!("{pid}.1"),
                        zombie: false,
                    },
                )
            })
            .collect();
        move |pid| links.iter().find(|(p, _)| *p == pid).map(|(_, i)| i.clone())
    }

    /// ADR-0035 D4: a harness's shared server holds every session, so a
    /// hook under it names no session's process, not the server and not
    /// the program that started the server above it.
    #[test]
    fn a_shared_server_is_not_a_sessions_process() {
        let server = ["app-server".to_string()];
        let under_server = chain(&[
            (30, "sh", &["hook.sh"]),
            (20, "codex", &["app-server", "--listen", "unix://"]),
            (10, "codex", &["--yolo"]),
        ]);
        assert_eq!(walk(30, "codex", &server, &under_server), None);
        let under_its_own = chain(&[(30, "sh", &["hook.sh"]), (10, "codex", &["--yolo"])]);
        assert_eq!(
            walk(30, "codex", &server, &under_its_own),
            Some(Process { pid: 10, start: "10.1".into() })
        );
        // A harness file that names no server: the first program found, as before.
        assert_eq!(walk(30, "codex", &[], &under_server).map(|p| p.pid), Some(20));
    }

    #[test]
    fn a_process_that_is_not_there_says_nothing() {
        assert_eq!(info(u32::MAX - 7), None);
    }

    /// A recorded process runs while its pid has its start time; one
    /// that exited, even unreaped, or a pid reused, does not.
    #[test]
    fn a_recorded_process_is_alive_only_as_itself() {
        use weft_core::sessions::Process;
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let start = info(child.id()).unwrap().start;
        let me = Process { pid: child.id(), start: start.clone() };
        assert!(alive(&me));
        assert!(!alive(&Process { pid: child.id(), start: format!("{start}0") }), "reused");
        child.kill().unwrap();
        // Killed and not yet waited for: a zombie, which is gone.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while alive(&me) {
            assert!(std::time::Instant::now() < deadline, "a zombie still read as alive");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.wait().unwrap();
        assert!(!alive(&me), "reaped");
    }
}
