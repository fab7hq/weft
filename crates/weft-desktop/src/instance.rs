//! One app per person: the first listens on a socket beside the daemon's,
//! and a second hands it the project it was started on, then ends. Two apps
//! would each write the same state, and the last to quit would win.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// Where the running app listens.
pub fn socket_path() -> PathBuf {
    weft_proto::socket_path().with_file_name("app.sock")
}

/// Hand `project` (or nothing, to be brought in front) to the app listening
/// at `socket`. Whether one took it.
pub fn hand_over(socket: &Path, project: Option<&Path>) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else { return false };
    let line = project.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    stream.write_all(format!("{line}\n").as_bytes()).and_then(|()| stream.flush()).is_ok()
}

/// Listen at `socket` for apps started after this one, on a thread of its
/// own: `taken` gets each one's project, or `None` when it named none.
/// Nothing when another app already listens there.
pub fn listen(
    socket: &Path,
    taken: impl Fn(Option<PathBuf>) + Send + 'static,
) -> std::io::Result<Listening> {
    weft_proto::clear_dead(socket);
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let listener = UnixListener::bind(socket)?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok() {
                let line = line.trim_end_matches('\n');
                taken((!line.is_empty()).then(|| PathBuf::from(line)));
            }
        }
    });
    Ok(Listening(socket.to_path_buf()))
}

/// This app's place to be handed projects; removed when it goes, so the next
/// app to start listens there itself.
pub struct Listening(PathBuf);

impl Drop for Listening {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn a_second_app_hands_its_project_to_the_first() {
        let socket = weft_proto::private_socket("weft-app-instance");
        assert!(!hand_over(&socket, None), "nothing listens yet");
        let (tx, rx) = channel();
        let held = listen(&socket, move |p| tx.send(p).unwrap()).unwrap();
        assert!(hand_over(&socket, Some(Path::new("/work/a b"))));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some("/work/a b".into()));
        assert!(hand_over(&socket, None));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), None, "brought in front");
        drop(held);
        assert!(!socket.exists(), "the next app listens in its place");
    }

    #[test]
    fn a_socket_left_by_an_app_that_died_is_taken_over() {
        let socket = weft_proto::private_socket("weft-app-dead");
        drop(UnixListener::bind(&socket).unwrap());
        assert!(socket.exists(), "left behind");
        let (tx, rx) = channel();
        let _held = listen(&socket, move |p| tx.send(p).unwrap()).unwrap();
        assert!(hand_over(&socket, Some(Path::new("/w"))));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some("/w".into()));
    }
}
