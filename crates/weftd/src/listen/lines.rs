//! One JSON message per line.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub(super) struct Lines {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Lines {
    pub(super) fn open(stream: UnixStream) -> std::io::Result<Self> {
        Ok(Lines { writer: stream.try_clone()?, reader: BufReader::new(stream) })
    }
}

impl super::Framed for Lines {
    fn send(&mut self, message: &str) -> std::io::Result<()> {
        writeln!(self.writer, "{message}")
    }

    fn next(&mut self) -> std::io::Result<Option<String>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(line.trim_end_matches('\n').to_string()))
    }
}
