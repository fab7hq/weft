//! A recording HTTP server standing in for GitHub, Fab7's account service
//! and push services: each request is kept, and answered from the routes a
//! test gives, in order, the last answer repeating.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

type Routes = Arc<Mutex<HashMap<String, Vec<(u16, String)>>>>;

pub struct Fake {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    routes: Routes,
}

impl Fake {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let routes: Routes = Arc::default();
        let (s, r) = (seen.clone(), routes.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (s, r) = (s.clone(), r.clone());
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let mut parts = line.split_whitespace();
                    let method = parts.next().unwrap_or("").to_string();
                    let path = parts.next().unwrap_or("").to_string();
                    let mut headers = HashMap::new();
                    loop {
                        let mut h = String::new();
                        reader.read_line(&mut h).unwrap();
                        let h = h.trim_end();
                        if h.is_empty() {
                            break;
                        }
                        if let Some((k, v)) = h.split_once(':') {
                            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
                        }
                    }
                    let length =
                        headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let key = format!("{method} {path}");
                    let (status, answer) = {
                        let mut routes = r.lock().unwrap();
                        match routes.get_mut(&key) {
                            Some(list) if list.len() > 1 => list.remove(0),
                            Some(list) if !list.is_empty() => list[0].clone(),
                            _ => (404, "{\"code\":\"not_found\"}".to_string()),
                        }
                    };
                    s.lock().unwrap().push(Seen { method, path, headers, body });
                    let mut out = stream;
                    let _ = write!(
                        out,
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                        answer.len()
                    );
                });
            }
        });
        Fake { url, seen, routes }
    }

    /// Answer `method path` with these, in turn; the last repeats.
    pub fn route(&self, method: &str, path: &str, answers: &[(u16, &str)]) {
        let list = answers.iter().map(|(s, a)| (*s, a.to_string())).collect();
        self.routes.lock().unwrap().insert(format!("{method} {path}"), list);
    }

    pub fn requests(&self, method: &str, path: &str) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.method == method && s.path == path)
            .cloned()
            .collect()
    }
}
