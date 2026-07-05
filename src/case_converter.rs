// src/case_converter.rs
#[derive(Clone, Copy, Debug)]
pub enum CaseType {
    Title,
    Upper,
    Lower,
    Sentence,
}

pub fn apply_case(text: &str, case_type: CaseType) -> String {
    match case_type {
        CaseType::Title => {
            text.split_whitespace()
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
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
    }
}
