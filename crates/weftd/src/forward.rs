//! weftd forwards evidence to `ringframe observe` (ADR-0030 D9): per project,
//! one call at a time in the order the hooks fired, so the ledger keeps their
//! order. In memory only: a forwarded prompt is in weftd between its signal
//! and its call, and nowhere after. A failed call is logged, without its
//! prompt, and dropped; it is never retried.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};

use serde_json::Value;
use weft_core::signal::Sent;

use crate::outside::Shared;

/// One queue per project, each drained by its own thread.
pub struct Forwarders {
    out: Shared,
    queues: HashMap<PathBuf, Sender<Sent>>,
}

impl Forwarders {
    pub fn new(out: Shared) -> Self {
        Forwarders { out, queues: HashMap::new() }
    }

    /// Queue a signal whose row forwards something.
    pub fn send(&mut self, root: &Path, sent: Sent) {
        if sent.forward.is_none() {
            return;
        }
        let out = self.out.clone();
        let tx = self.queues.entry(root.to_path_buf()).or_insert_with(|| {
            let (tx, rx) = channel::<Sent>();
            let root = root.to_path_buf();
            std::thread::spawn(move || {
                let mut tools: HashMap<String, Value> = HashMap::new();
                for sent in rx {
                    drain(&*out, &root, &sent, &mut tools);
                }
            });
            tx
        });
        let _ = tx.send(sent);
    }
}

/// One forwarded signal: nothing without a record here, a tool only when its
/// harness's profile names it, read once per harness.
fn drain(
    out: &dyn crate::outside::Outside,
    root: &Path,
    sent: &Sent,
    tools: &mut HashMap<String, Value>,
) {
    if !root.join(".fab7").join("rf").is_dir() {
        return;
    }
    let observe = if sent.forward.as_deref() == Some("tool") {
        if !tools.contains_key(&sent.harness)
            && let Some(t) = crate::ringframe::observe_tools(out, root, &sent.harness)
        {
            tools.insert(sent.harness.clone(), t);
        }
        tools.get(&sent.harness).cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let Some(call) = weft_core::signal::forward(sent, &observe) else { return };
    if let Err(e) = crate::ringframe::observe(out, root, &call) {
        let code = match e {
            crate::ringframe::Error::NotInstalled => "not installed".to_string(),
            crate::ringframe::Error::Refused { code, .. } => format!("exit {code}"),
        };
        eprintln!("weft: ringframe {} {}: {code}", call.args[0], call.args[1]);
    }
}
