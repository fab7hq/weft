//! Replay a captured terminal stream and print the screen it produced.
//!
//!   cargo run --example replay -- /tmp/weft-shot-board.bin 24 100

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("a capture file");
    let rows: u16 = a.next().and_then(|s| s.parse().ok()).unwrap_or(24);
    let cols: u16 = a.next().and_then(|s| s.parse().ok()).unwrap_or(100);
    let bytes = std::fs::read(&path).expect("read");
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(&bytes);
    println!("{}", parser.screen().contents());
}
