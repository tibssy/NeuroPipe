use std::sync::LazyLock as Lazy;

use fancy_regex::Regex;

static SENTENCE_END: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[.!?](?:\s|$)").expect("sentence end regex"));

struct Rule {
    pat: &'static str,
    repl: &'static str,
    multiline: bool,
}

// Order matters: bold before italic using the same marker char; the
// lookaround-based underscore italic requires fancy_regex.
const RULES: &[Rule] = &[
    Rule { pat: r"\[([^\]]*)\]\([^)]+\)", repl: "$1", multiline: false }, // [text](url)
    Rule { pat: r"!\[([^\]]*)\]\([^)]+\)", repl: "$1", multiline: false }, // ![alt](url)
    Rule { pat: r"```[\s\S]*?```", repl: "", multiline: false }, // code blocks
    Rule { pat: r"`([^`]+)`", repl: "$1", multiline: false }, // inline code
    Rule { pat: r"\*\*([^*]+)\*\*", repl: "$1", multiline: false }, // **bold**
    Rule { pat: r"\*([^*]+)\*", repl: "$1", multiline: false }, // *italic*
    Rule { pat: r"__([^_]+)__", repl: "$1", multiline: false }, // __bold__
    Rule { pat: r"(?<!\w)_([^_]+)_(?!\w)", repl: "$1", multiline: false }, // _italic_
    Rule { pat: r"~~([^~]+)~~", repl: "$1", multiline: false }, // ~~strikethrough~~
    Rule { pat: r"^#+\s*", repl: "", multiline: true }, // # headings
    Rule { pat: r"^>\s*", repl: "", multiline: true }, // > blockquotes
    Rule { pat: r"^[-*+]\s+", repl: "", multiline: true }, // - list items
    Rule { pat: r"^\d+\.\s+", repl: "", multiline: true }, // 1. list items
    Rule { pat: r"^[-*_]{3,}\s*$", repl: "", multiline: true }, // hr
    Rule { pat: r"<[^>]+>", repl: "", multiline: false }, // <html> or <url>
];

fn pattern(rule: &Rule) -> String {
    if rule.multiline {
        format!("(?m){}", rule.pat)
    } else {
        rule.pat.to_string()
    }
}

pub fn strip_markdown(text: &str) -> String {
    let mut s = text.to_string();
    for rule in RULES {
        let re = Regex::new(&pattern(rule)).expect("valid rule regex");
        s = re.replace_all(&s, rule.repl).into_owned();
    }
    s.trim().to_string()
}

/// Splits off one complete sentence (per the Python SENTENCE_END rule).
/// Returns (sentence, byte_offset_after_match) if a complete sentence is found.
pub fn next_sentence(buffer: &str) -> Option<(String, usize)> {
    // SENTENCE_END = [.!?](?:\s|$)
    match SENTENCE_END.find(buffer) {
        Ok(Some(m)) => {
            let end = min_char_boundary(buffer, m.end());
            let sentence = buffer[..end].trim().to_string();
            Some((sentence, end))
        }
        _ => None,
    }
}

/// Snap a byte offset to the nearest char boundary at or before it, so slicing
/// can never panic on a multi-byte char.
fn min_char_boundary(s: &str, byte: usize) -> usize {
    let mut i = byte.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}