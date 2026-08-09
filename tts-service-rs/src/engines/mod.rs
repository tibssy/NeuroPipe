pub mod kokoro;
pub mod pocket_tts;
pub mod supertonic;

use anyhow::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Low,
    High,
}

pub trait TtsEngine: Send {
    fn load(&mut self) -> Result<()>;
    fn unload(&mut self);
    fn set_quality(&mut self, quality: Quality) -> Result<()>;
    fn voices(&mut self) -> Result<Vec<String>>;
    fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<(Vec<f32>, u32)>;
}

const ABBREVIATIONS: &[&str] = &[
    "Mr", "Mrs", "Ms", "Miss", "Dr", "Prof", "St", "Ave", "Blvd", "Rd", "Jr", "Sr", "vs", "etc",
    "e.g", "i.e", "al", "Capt", "Sgt", "Lt", "Col", "Gen", "Mt", "Ft", "U.S", "U.K", "D.C", "Ph.D",
    "M.D", "B.A", "M.A",
];

fn is_abbreviation(token: &str) -> bool {
    let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    ABBREVIATIONS
        .iter()
        .any(|abbr| abbr.eq_ignore_ascii_case(token))
}

fn is_decimal_point(text: &str, index: usize, character: char) -> bool {
    if character != '.' {
        return false;
    }
    let prev = text[..index].chars().next_back();
    let next = text[index + character.len_utf8()..].chars().next();
    matches!((prev, next), (Some(c), Some(n)) if c.is_ascii_digit() && n.is_ascii_digit())
}

fn is_initial(text: &str, index: usize, character: char) -> bool {
    if character != '.' {
        return false;
    }
    let before = text[..index].trim_end();
    let Some(previous) = before.chars().next_back() else {
        return false;
    };
    if !previous.is_ascii_uppercase() {
        return false;
    }
    // A single uppercase letter preceded by whitespace or a quote is an initial.
    let mut chars = before.chars();
    chars.next_back();
    match chars.next_back() {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '"' | '\'' | '(' | '['),
    }
}

fn is_terminator(text: &str, index: usize, character: char) -> bool {
    match character {
        '!' | '?' => true,
        '\n' => true,
        '.' => {
            let before = &text[..index];
            let rest = &text[index + 1..];
            let previous_char = before.chars().next_back();
            if matches!(previous_char, Some('.')) || rest.starts_with('.') {
                return false; // ellipsis "..."
            }
            if is_decimal_point(text, index, character) || is_initial(text, index, character) {
                return false;
            }
            // A period followed by a lowercase letter or digit (after spaces)
            // continues the sentence: "e.g. here", "item 1. is", "3.14".
            if let Some(next) = rest.trim_start().chars().next() {
                if next.is_lowercase() || next.is_ascii_digit() {
                    return false;
                }
            }
            // A period ending a known abbreviation is not a boundary.
            match before.trim_end().split_whitespace().next_back() {
                Some(previous) => !is_abbreviation(previous),
                None => true,
            }
        }
        _ => false,
    }
}

pub fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut start = 0;
    for (index, character) in text.char_indices() {
        if is_terminator(text, index, character) {
            let mut end = index + character.len_utf8();
            // Consume trailing closers so quotes/brackets stay with the sentence.
            for trailing in text[end..].chars() {
                if matches!(trailing, '"' | '\'' | ')' | ']' | '}' | '»' | '”') {
                    end += trailing.len_utf8();
                } else {
                    break;
                }
            }
            let sentence = text[start..end].trim();
            if !sentence.is_empty() {
                sentences.push(sentence.to_string());
            }
            start = end;
        }
    }
    let remainder = text[start..].trim();
    if !remainder.is_empty() {
        sentences.push(remainder.to_string());
    }
    if sentences.is_empty() {
        vec![text.trim().to_string()]
    } else {
        sentences
    }
}

#[cfg(test)]
mod tests {
    use super::split_sentences;

    #[test]
    fn splits_sentences_without_losing_punctuation() {
        assert_eq!(
            split_sentences("First sentence. Second sentence!"),
            vec!["First sentence.", "Second sentence!"]
        );
    }

    #[test]
    fn does_not_split_on_abbreviations() {
        assert_eq!(
            split_sentences("Dr. Smith is here. He left."),
            vec!["Dr. Smith is here.", "He left."]
        );
        assert_eq!(
            split_sentences("Use e.g. or i.e. in prose. Done."),
            vec!["Use e.g. or i.e. in prose.", "Done."]
        );
        assert_eq!(
            split_sentences("The U.S. is big. Really."),
            vec!["The U.S. is big.", "Really."]
        );
    }

    #[test]
    fn does_not_split_on_decimals_or_numbers() {
        assert_eq!(
            split_sentences("Pi is 3.14. Yes."),
            vec!["Pi is 3.14.", "Yes."]
        );
        assert_eq!(
            split_sentences("Version 1.2 shipped. Great."),
            vec!["Version 1.2 shipped.", "Great."]
        );
    }

    #[test]
    fn does_not_split_on_ellipses() {
        assert_eq!(split_sentences("Wait... really?"), vec!["Wait... really?"]);
    }

    #[test]
    fn does_not_split_on_initials() {
        assert_eq!(
            split_sentences("J. R. R. Tolkien wrote it."),
            vec!["J. R. R. Tolkien wrote it."]
        );
    }

    #[test]
    fn keeps_closing_quotes_with_sentence() {
        assert_eq!(
            split_sentences("He said \"hi.\" Then left."),
            vec!["He said \"hi.\"", "Then left."]
        );
    }

    #[test]
    fn splits_on_newlines() {
        assert_eq!(
            split_sentences("Line one.\nLine two."),
            vec!["Line one.", "Line two."]
        );
    }
}
