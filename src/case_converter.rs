// src/case_converter.rs
#[derive(Clone, Copy, Debug)]
pub enum CaseType {
    Title,
    Upper,
    Lower,
    Sentence,
    /// Flips each letter's case ("tOGGLE cASE").
    Toggle,
}

pub fn apply_case(text: &str, case_type: CaseType) -> String {
    match case_type {
        // A char scan, not `split_whitespace().join(" ")` — the split/join
        // form drops leading/trailing whitespace and collapses runs of
        // whitespace to a single space, which (applied per-run, as this is)
        // silently glues adjacent runs' words together.
        CaseType::Title => {
            let mut result = String::new();
            let mut capitalize_next = true;
            for ch in text.chars() {
                if capitalize_next && ch.is_alphabetic() {
                    result.push_str(&ch.to_uppercase().to_string());
                } else {
                    result.push(ch);
                }
                // Whitespace-only boundary — same word definition
                // `split_whitespace()` used, so `don't`/`o'brien` don't get
                // capitalized mid-word the way a "non-alphanumeric" boundary
                // would.
                capitalize_next = ch.is_whitespace();
            }
            result
        }
        CaseType::Upper => text.to_uppercase(),
        CaseType::Lower => text.to_lowercase(),
        CaseType::Sentence => {
            let mut result = String::new();
            let mut capitalize_next = true;
            for ch in text.chars() {
                if capitalize_next && ch.is_alphabetic() {
                    result.push_str(&ch.to_uppercase().to_string());
                    capitalize_next = false;
                } else {
                    result.push(ch);
                    if matches!(ch, '.' | '!' | '?') {
                        capitalize_next = true;
                    }
                }
            }
            result
        }
        CaseType::Toggle => text
            .chars()
            .map(|ch| {
                if ch.is_uppercase() {
                    ch.to_lowercase().collect::<String>()
                } else if ch.is_lowercase() {
                    ch.to_uppercase().collect::<String>()
                } else {
                    ch.to_string()
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_preserves_whitespace() {
        assert_eq!(apply_case("hello  world ", CaseType::Title), "Hello  World ");
    }

    #[test]
    fn title_does_not_capitalize_after_an_apostrophe() {
        assert_eq!(apply_case("don't stop o'brien", CaseType::Title), "Don't Stop O'brien");
    }

    #[test]
    fn upper() {
        assert_eq!(apply_case("Hello World", CaseType::Upper), "HELLO WORLD");
    }

    #[test]
    fn lower() {
        assert_eq!(apply_case("Hello World", CaseType::Lower), "hello world");
    }

    #[test]
    fn sentence() {
        assert_eq!(
            apply_case("hello world. another one", CaseType::Sentence),
            "Hello world. Another one"
        );
    }

    #[test]
    fn toggle() {
        assert_eq!(apply_case("Hello World! 123", CaseType::Toggle), "hELLO wORLD! 123");
    }
}
