//! An agent's conversation, as it goes to a device (remote.md §6d): a page
//! fits one WebSocket message, and an item too long for one goes in parts
//! the device joins by id. Nothing is cut, and nothing is kept here but a
//! followed agent's cursor.

use serde_json::{Value, json};
use weft_core::conversation::Cursor;

/// The most of an item's text one part holds.
pub const PART: usize = 32 * 1024;
/// The most one message to a device holds.
pub const MESSAGE: usize = 60 * 1024;

/// Each item, or its parts when its text is longer than `PART`: the same id,
/// `part` from 1 and `parts`.
pub fn pieces(items: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for item in items {
        let text = item["text"].as_str().unwrap_or_default();
        if text.len() <= PART {
            out.push(item.clone());
            continue;
        }
        let mut chunks = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let mut end = rest.len().min(PART);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            chunks.push(&rest[..end]);
            rest = &rest[end..];
        }
        for (n, chunk) in chunks.iter().enumerate() {
            let mut part = item.clone();
            part["text"] = (*chunk).into();
            part["part"] = (n + 1).into();
            part["parts"] = chunks.len().into();
            out.push(part);
        }
    }
    out
}

fn size(v: &Value) -> usize {
    v.to_string().len()
}

/// A page as a device gets it: the `conversation` its `ok` carries, then the
/// earlier parts of an item the page holds only some of, a message each. A
/// page that does not fit one message ends early, with at least one item or
/// part, and its `before` is that item's line.
pub fn page(answer: &Value, agent: &str) -> (Value, Vec<Value>) {
    let mut page = answer.clone();
    let items = answer["items"].as_array().cloned().unwrap_or_default();
    let all = pieces(&items);
    let mut room = MESSAGE.saturating_sub(size(&page) + 64);
    let mut first = all.len();
    while first > 0 && (first == all.len() || size(&all[first - 1]) < room) {
        room = room.saturating_sub(size(&all[first - 1]) + 1);
        first -= 1;
    }
    let kept = &all[first..];
    // An item cut by the page: its earlier parts follow, one message each.
    let mut rest = Vec::new();
    if let Some(cut) = kept.first().filter(|p| p["part"].as_u64().is_some_and(|n| n > 1)) {
        for part in all[..first].iter().filter(|p| p["id"] == cut["id"]) {
            rest.push(json!({"type": "conversation", "agent": agent, "items": [part]}));
        }
    }
    if first > 0 {
        let after = answer["after"].as_str().and_then(Cursor::of);
        let at = kept.first().and_then(|p| p["id"].as_str());
        if let (Some(after), Some(at)) = (after, at) {
            page["before"] = after.at_item(at).map(Cursor::text).into();
        }
    }
    page["items"] = kept.to_vec().into();
    (page, rest)
}

/// What a followed agent's file grew by, as `conversation` messages that
/// each fit one: the cursor goes on the last.
pub fn grown(items: &[Value], after: &str, agent: &str) -> Vec<Value> {
    let empty = json!({"type": "conversation", "agent": agent, "items": []});
    let room = MESSAGE.saturating_sub(size(&empty) + after.len() + 16);
    let mut messages: Vec<Vec<Value>> = vec![Vec::new()];
    let mut used = 0;
    for piece in pieces(items) {
        let n = size(&piece) + 1;
        if used + n > room && !messages.last().expect("one").is_empty() {
            messages.push(Vec::new());
            used = 0;
        }
        used += n;
        messages.last_mut().expect("one").push(piece);
    }
    let last = messages.len() - 1;
    messages
        .into_iter()
        .enumerate()
        .map(|(n, items)| {
            let mut m = json!({"type": "conversation", "agent": agent, "items": items});
            if n == last {
                m["after"] = after.into();
            }
            m
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, text: &str) -> Value {
        json!({"id": id, "kind": "agent", "at": 1, "text": text})
    }

    /// Joined as a device joins them: by id, in part order.
    fn joined(pieces: &[Value]) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for p in pieces {
            let id = p["id"].as_str().unwrap().to_string();
            let text = p["text"].as_str().unwrap();
            match out.iter_mut().find(|(i, _)| *i == id) {
                Some((_, t)) => t.push_str(text),
                None => out.push((id, text.to_string())),
            }
        }
        out
    }

    #[test]
    fn a_long_message_goes_in_parts_and_nothing_is_cut() {
        let long = "é".repeat(PART);
        let all = pieces(&[item("0.0", "hi"), item("10.0", &long)]);
        assert_eq!(all.len(), 3);
        assert_eq!((all[1]["part"].as_u64(), all[2]["parts"].as_u64()), (Some(1), Some(2)));
        assert_eq!(joined(&all)[1].1, long);
    }

    #[test]
    fn a_page_too_big_for_one_message_ends_early_with_its_line_as_before() {
        let items: Vec<Value> =
            (0..30).map(|n| item(&format!("{}.0", n * 100), &"x".repeat(4_000))).collect();
        let answer = json!({"readable": true, "items": items, "before": null, "after": "7-3000"});
        let (page, rest) = page(&answer, "a-0");
        let kept = page["items"].as_array().unwrap();
        assert!(kept.len() < 30 && !kept.is_empty() && rest.is_empty());
        assert!(page.to_string().len() <= MESSAGE);
        assert_eq!(kept.last().unwrap()["id"], "2900.0", "newest last");
        let first = kept[0]["id"].as_str().unwrap().split('.').next().unwrap();
        assert_eq!(page["before"], format!("7-{first}"));
        let small =
            json!({"readable": true, "items": [item("0.0", "hi")], "before": null, "after": "7-9"});
        assert_eq!(super::page(&small, "a-0").0["before"], Value::Null);
    }

    #[test]
    fn a_page_holding_part_of_an_item_sends_its_earlier_parts_after() {
        let long = "y".repeat(PART * 3);
        let answer = json!({"readable": true, "items": [item("0.0", "hi"), item("5.0", &long)],
                            "before": null, "after": "7-9"});
        let (page, rest) = page(&answer, "a-0");
        let mut all: Vec<Value> = page["items"].as_array().unwrap().clone();
        assert!(page.to_string().len() <= MESSAGE);
        for m in &rest {
            assert!(m.to_string().len() <= MESSAGE);
            all.extend(m["items"].as_array().unwrap().iter().cloned());
        }
        all.sort_by_key(|p| p["part"].as_u64());
        assert_eq!(joined(&all), [("5.0".to_string(), long)]);
        assert_eq!(page["before"], "7-5", "the item cut, then what came before it");
    }

    #[test]
    fn what_grew_fits_messages_and_the_cursor_is_on_the_last() {
        let items: Vec<Value> =
            (0..40).map(|n| item(&format!("{n}.0"), &"z".repeat(3_000))).collect();
        let messages = grown(&items, "7-99", "a-0");
        assert!(messages.len() > 1);
        assert!(messages.iter().all(|m| m.to_string().len() <= MESSAGE));
        assert!(messages[..messages.len() - 1].iter().all(|m| m.get("after").is_none()));
        assert_eq!(messages.last().unwrap()["after"], "7-99");
        let n: usize = messages.iter().map(|m| m["items"].as_array().unwrap().len()).sum();
        assert_eq!(n, 40);
    }
}
