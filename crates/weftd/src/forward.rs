//! weftd forwards evidence to `ringframe observe`: per project,
//! one call at a time in the order the hooks fired, so the ledger keeps their
//! order. In memory only: a forwarded prompt is in weftd between its signal
//! and its call, and nowhere after. A failed call is logged, without its
//! prompt, and dropped; it is never retried. A prompt that answers Weft's own
//! send hands RingFrame's answer back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};

use serde_json::Value;
use weft_core::signal::Sent;

use crate::outside::Shared;

/// What RingFrame answered a forwarded prompt, for the send it answers: its
/// answer, or why there is none.
pub type Answered = Box<dyn FnOnce(Result<Value, String>) + Send>;

/// One queue per project, each drained by its own thread.
pub struct Forwarders {
    out: Shared,
    queues: HashMap<PathBuf, Sender<(Sent, Option<Answered>)>>,
}

impl Forwarders {
    pub fn new(out: Shared) -> Self {
        Forwarders { out, queues: HashMap::new() }
    }

    /// Queue a signal whose row forwards something; `answered`, when given,
    /// is told what RingFrame made of it.
    pub fn send(&mut self, root: &Path, sent: Sent, answered: Option<Answered>) {
        if sent.forward.is_none() {
            return;
        }
        let out = self.out.clone();
        let tx = self.queues.entry(root.to_path_buf()).or_insert_with(|| {
            let (tx, rx) = channel::<(Sent, Option<Answered>)>();
            let root = root.to_path_buf();
            std::thread::spawn(move || {
                let mut tools: HashMap<String, Value> = HashMap::new();
                for (sent, answered) in rx {
                    let answer = drain(&*out, &root, &sent, &mut tools);
                    if let Some(answered) = answered {
                        answered(answer);
                    }
                }
            });
            tx
        });
        let _ = tx.send((sent, answered));
    }
}

/// One forwarded signal: nothing without a record here, a tool only when its
/// harness's profile names it, read once per harness. RingFrame's answer, or
/// why there is none.
fn drain(
    out: &dyn crate::outside::Outside,
    root: &Path,
    sent: &Sent,
    tools: &mut HashMap<String, Value>,
) -> Result<Value, String> {
    if !root.join(".fab7").join("rf").is_dir() {
        return Err("no record here".into());
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
    let Some(call) = weft_core::signal::forward(sent, &observe) else {
        return Err("nothing to forward".into());
    };
    crate::ringframe::observe(out, root, &call).map_err(|e| {
        let code = match e {
            crate::ringframe::Error::NotInstalled => "not installed".to_string(),
            crate::ringframe::Error::Refused { code, .. } => format!("exit {code}"),
        };
        eprintln!("weft: ringframe {} {}: {code}", call.args[0], call.args[1]);
        format!("ringframe {code}")
    })
}
