//! Spellchecking: tokenizing paragraph text, deciding which tokens are worth
//! checking at all, and looking the survivors up in a Hunspell dictionary.
//!
//! Deliberately gpui-free (same convention as `state.rs`) — it returns plain
//! char-column ranges and `String`s, and the view layer decides how to paint
//! them. `notes/spellcheck_plan.md` has the scope decisions behind it.

use std::collections::HashSet;
use std::sync::OnceLock;

use spellbook::Dictionary;

/// The dictionary is baked into the binary rather than read from disk next to
/// the executable (which is what `settings_conf_path()` does for settings).
/// `include_str!` costs ~848K of binary and in exchange there is no path to
/// resolve, no `[package.metadata.bundle] resources` entry to keep in sync,
/// and no "dictionary file missing" failure mode on a tester's machine — the
/// checker either works or the build failed. The files are the standard
/// SCOWL-derived en_US Hunspell pair; see `assets/en_US.LICENSE`.
const EN_US_AFF: &str = include_str!("../assets/en_US.aff");
const EN_US_DIC: &str = include_str!("../assets/en_US.dic");

/// Max suggestions shown in the right-click menu. More than this and the menu
/// is taller than it is useful; Word shows a comparable handful.
pub const MAX_SUGGESTIONS: usize = 5;

/// Parsed once on first use.
///
/// ponytail: the ~50ms parse lands on whichever frame first paints text.
/// Move it to a `cx.background_executor()` task at startup if that ever shows
/// up as a visible stall.
fn dict() -> Option<&'static Dictionary> {
    static DICT: OnceLock<Option<Dictionary>> = OnceLock::new();
    DICT.get_or_init(|| match Dictionary::new(EN_US_AFF, EN_US_DIC) {
        Ok(d) => Some(d),
        // A corrupt vendored dictionary shouldn't take the editor down with
        // it — spellcheck just goes quiet.
        Err(e) => {
            crate::state::log_line(&format!("[spellcheck] failed to parse bundled en_US dictionary: {e}"));
            None
        }
    })
    .as_ref()
}

/// Char-column ranges of the misspelled words in one paragraph's text.
///
/// Ranges are char columns, not byte offsets, because that is the unit
/// `render_line`/`line_segments` already work in — converting at the render
/// boundary instead would mean doing it per row, per frame.
pub fn misspelled_ranges(text: &str, user_dict: &HashSet<String>) -> Vec<(usize, usize)> {
    let Some(dict) = dict() else { return Vec::new() };
    tokenize(text)
        .into_iter()
        .filter(|tok| {
            !user_dict.contains(&tok.text.to_lowercase()) && !dict.check(&tok.text)
        })
        .map(|tok| (tok.start, tok.end))
        .collect()
}

/// Replacement candidates for a misspelled word, best first, capped at
/// `MAX_SUGGESTIONS`.
///
/// Only ever called from the right-click handler, never from `render` —
/// `suggest` is a dictionary *search* and is orders of magnitude slower than
/// `check`, so it must stay off the frame budget.
pub fn suggest(word: &str) -> Vec<String> {
    let Some(dict) = dict() else { return Vec::new() };
    let mut out = Vec::new();
    dict.suggest(word, &mut out);
    out.truncate(MAX_SUGGESTIONS);
    out
}

/// A word worth checking, with its char-column span in the source text.
struct Token {
    text: String,
    start: usize,
    end: usize,
}

/// Splits `text` into checkable words, dropping the ones the skip rules
/// reject (`notes/spellcheck_plan.md` Phase 1).
///
/// A word is a run of alphabetic chars plus internal `'`/`-` (so "don't" and
/// "cost-benefit" stay whole rather than fragmenting into false positives).
/// Trailing apostrophes/hyphens are trimmed back off — "the '90s" shouldn't
/// make the quote part of the word.
fn tokenize(text: &str) -> Vec<Token> {
    // Debate cites are full of bare URLs. One `://` anywhere in the paragraph
    // is enough to make per-token URL detection unreliable (the host, path,
    // and TLD all tokenize as separate words), so the whole paragraph opts
    // out — matching the plan's "digits/URLs/punctuation" rule at the level
    // it can actually be enforced.
    if text.contains("://") {
        return Vec::new();
    }

    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    // The first alphabetic token of the paragraph is sentence-initial, as is
    // the first one after any `.`/`!`/`?`. Drives the capitalized-mid-sentence
    // skip below.
    let mut sentence_initial = true;

    while i < chars.len() {
        let c = chars[i];
        if !is_word_char(c) {
            if matches!(c, '.' | '!' | '?') {
                sentence_initial = true;
            }
            i += 1;
            continue;
        }

        let start = i;
        while i < chars.len() && is_word_char(chars[i]) {
            i += 1;
        }
        // Trim leading/trailing joiners — they're only meaningful *between*
        // letters.
        let mut s = start;
        let mut e = i;
        while s < e && !chars[s].is_alphabetic() { s += 1; }
        while e > s && !chars[e - 1].is_alphabetic() { e -= 1; }
        if s == e {
            continue;
        }

        let word: String = chars[s..e].iter().collect();
        let was_sentence_initial = sentence_initial;
        sentence_initial = false;

        if is_checkable(&word, was_sentence_initial) {
            tokens.push(Token { text: word, start: s, end: e });
        }
    }
    tokens
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic() || c == '\'' || c == '\u{2019}' || c == '-'
}

/// The skip rules. Returning `false` means "don't even ask the dictionary".
fn is_checkable(word: &str, sentence_initial: bool) -> bool {
    // Single letters are initials, list markers, or the words "a"/"I" — never
    // worth a squiggle.
    if word.chars().count() < 2 {
        return false;
    }
    // Digits mean a cite, a date, or a statistic, not prose.
    if word.chars().any(|c| c.is_ascii_digit()) || word.contains('@') {
        return false;
    }

    let mut cs = word.chars();
    let first_upper = cs.next().is_some_and(|c| c.is_uppercase());
    // Author surnames are the single largest source of false positives in a
    // card doc, and they're overwhelmingly capitalized mid-sentence.
    //
    // The `!all_upper` clause is load-bearing: ALL-CAPS words start uppercase
    // too, so without it this rule would silently stop checking every tag
    // line — the opposite of what's wanted, since tags are short and
    // hand-typed and that's exactly where typos live.
    let all_upper = word.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase());
    if first_upper && !all_upper && !sentence_initial {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One table over the skip rules — the part of this module with branches
    /// worth protecting. Deliberately doesn't assert on dictionary contents
    /// beyond the obvious, so it can't fail on a dictionary bump.
    #[test]
    fn test_is_checkable_skip_rules() {
        // (word, sentence_initial, expected)
        let cases = [
            ("hello", false, true),
            ("a", true, false),                 // too short
            ("I", true, false),                 // too short
            ("covid19", false, false),          // has a digit
            ("me@example", false, false),       // has an @
            ("NATO", false, true),              // ALL-CAPS is still checked
            ("AFF", false, true),               // ...including short tags
            ("Kagan", false, false),            // capitalized mid-sentence
            ("Kagan", true, true),              // ...but not at sentence start
            ("Teh", true, true),                // sentence-start typo survives
            ("cost-benefit", false, true),
        ];
        for (word, initial, expected) in cases {
            assert_eq!(
                is_checkable(word, initial),
                expected,
                "is_checkable({word:?}, sentence_initial={initial})"
            );
        }
    }

    #[test]
    fn test_tokenize_spans_and_sentence_tracking() {
        // "Smith" is sentence-initial (skipped by nothing), "Jones" follows a
        // period so it's sentence-initial too, "Brown" is mid-sentence and
        // gets skipped by the capitalization rule.
        let toks = tokenize("Smith said. Jones and Brown");
        let words: Vec<&str> = toks.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(words, vec!["Smith", "said", "Jones", "and"]);

        // Spans are char columns into the source, not byte offsets.
        let toks = tokenize("don't");
        assert_eq!(toks.len(), 1);
        assert_eq!((toks[0].start, toks[0].end), (0, 5));
    }

    #[test]
    fn test_tokenize_skips_paragraphs_containing_urls() {
        assert!(tokenize("see https://example.com/x for more").is_empty());
    }

    #[test]
    fn test_tokenize_trims_trailing_joiners() {
        let toks = tokenize("the '90s were-");
        let words: Vec<&str> = toks.iter().map(|t| t.text.as_str()).collect();
        // "'90s" is dropped (digit), the trailing hyphen is trimmed off "were".
        assert_eq!(words, vec!["the", "were"]);
    }

    #[test]
    fn test_misspelled_ranges_flags_typos_and_honors_user_dict() {
        let empty = HashSet::new();
        let ranges = misspelled_ranges("hello wrold", &empty);
        assert_eq!(ranges, vec![(6, 11)], "expected 'wrold' flagged");

        let mut user = HashSet::new();
        user.insert("wrold".to_string());
        assert!(
            misspelled_ranges("hello wrold", &user).is_empty(),
            "user dictionary entry should suppress the squiggle"
        );
    }
}

