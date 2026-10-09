//! The harness process a hook ran under (ADR-0030 D8): the first ancestor of
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
    pub parent: u32,
    pub start: String,
    /// It has exited and is not yet reaped: gone, for anyone asking.
    pub zombie: bool,
}

/// Whether the process a session's receipts name still runs: that pid, with
/// that start time, and not a zombie (ADR-0029 D11).
pub fn alive(p: &weft_core::sessions::Process) -> bool {
    info(p.pid).is_some_and(|me| me.start == p.start && !me.zombie)
}

/// The first process at or above `from` whose name, or else whose
/// `argv[0]`'s base name, is `program`'s base name.
pub fn ancestor_named(from: u32, program: &str) -> Option<Process> {
    let want = base(program);
    let mut pid = from;
    // A chain of processes is short; this only stops a cycle from spinning.
    for _ in 0..64 {
        if pid == 0 {
            return None;
        }
        let me = info(pid)?;
        if me.name == want || me.argv0.as_deref() == Some(want) {
            return Some(Process { pid, start: me.start });
        }
        if me.parent == pid {
            return None;
        }
        pid = me.parent;
    }
    None
}

/// The harness process this one runs under.
pub fn harness(program: &str) -> Option<Process> {
    ancestor_named(std::os::unix::process::parent_id(), program)
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
    let argv0 = cmdline.split(|b| *b == 0).next().filter(|a| !a.is_empty());
    let argv0 = argv0.map(|a| base(&String::from_utf8_lossy(a)).to_string());
    Some(Info { name, argv0, parent, start, zombie })
}

/// `proc_pidinfo(PROC_PIDTBSDINFO)`: the name (`p_comm`), the parent and the
/// start time, seconds and microseconds; `argv[0]` from `KERN_PROCARGS2`.
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
    Some(Info {
        name: String::from_utf8_lossy(&comm).into_owned(),
        argv0: argv0(pid),
        parent: bsd.pbi_ppid,
        start: format!("{}.{:06}", bsd.pbi_start_tvsec, bsd.pbi_start_tvusec),
        // `SZOMB` in <sys/proc.h>.
        zombie: bsd.pbi_status == 5,
    })
}

/// `KERN_PROCARGS2` holds `argc`, the executable's path, padding, then argv.
#[cfg(target_os = "macos")]
fn argv0(pid: u32) -> Option<String> {
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
    let rest = &buf[4..len];
    let path_end = rest.iter().position(|b| *b == 0)?;
    let args = &rest[path_end..];
    let start = args.iter().position(|b| *b != 0)?;
    let arg = args[start..].split(|b| *b == 0).next()?;
    Some(base(&String::from_utf8_lossy(arg)).to_string())
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
        let found = ancestor_named(me, &up.name).expect("the parent, by name");
        assert_eq!(found, Process { pid: parent, start: up.start.clone() });
        // A path names its program by its base name.
        let path = format!("/somewhere/bin/{}", up.name);
        assert_eq!(ancestor_named(me, &path).map(|p| p.pid), Some(parent));
        assert_eq!(ancestor_named(me, "no-such-program-weft"), None);
    }

    #[test]
    fn a_process_that_is_not_there_says_nothing() {
        assert_eq!(info(u32::MAX - 7), None);
    }

    /// D11: a recorded process runs while its pid has its start time; one
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
