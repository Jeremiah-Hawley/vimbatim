//! GPUI-free editor key translation.

use crate::app::command::AppCommand;

/// Translates an editor keystroke into an application command when the active
/// editing mode owns it. Platform focus, scrolling, and clipboard work stay in
/// the view adapter.
pub(crate) trait InputStrategy {
    fn command(&self, key: &str, shift: bool, key_char: Option<&str>) -> Option<AppCommand>;
}

/// Shared editing commands for Vim Insert mode and Vim-disabled editing.
pub(crate) struct PlainInputStrategy;

impl PlainInputStrategy {
    /// Returns whether a plain editing command was executed.
    pub(crate) fn apply(
        &self,
        state: &mut crate::state::AppState,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        let Some(command) = self.command(key, shift, key_char) else {
            return false;
        };
        state.dispatch(command);
        true
    }
}

impl InputStrategy for PlainInputStrategy {
    fn command(&self, key: &str, shift: bool, key_char: Option<&str>) -> Option<AppCommand> {
        match key {
            "backspace" => Some(AppCommand::Backspace),
            "delete" => Some(AppCommand::DeleteForward),
            "enter" => Some(AppCommand::InsertChar('\n')),
            "space" => Some(AppCommand::InsertChar(' ')),
            "tab" => {
                if shift {
                    Some(AppCommand::OutdentListItem)
                } else {
                    Some(AppCommand::IndentListItem)
                }
            }
            "left" => {
                if shift {
                    Some(AppCommand::ExtendLeft)
                } else {
                    Some(AppCommand::MoveLeft)
                }
            }
            "right" => {
                if shift {
                    Some(AppCommand::ExtendRight)
                } else {
                    Some(AppCommand::MoveRight)
                }
            }
            _ if key_char.is_some_and(|text| text.chars().count() == 1) => {
                Some(AppCommand::InsertChar(key_char?.chars().next()?))
            }
            k if k.chars().count() == 1 => {
                let mut ch = k.chars().next().unwrap();
                if shift && ch.is_alphabetic() {
                    ch = ch.to_uppercase().next().unwrap_or(ch);
                }
                Some(AppCommand::InsertChar(ch))
            }
            _ => None,
        }
    }
}

/// Vim keystrokes always enter the application boundary as `AppCommand`s.
pub(crate) struct VimInputStrategy;

impl InputStrategy for VimInputStrategy {
    fn command(&self, key: &str, shift: bool, key_char: Option<&str>) -> Option<AppCommand> {
        (!key.is_empty()).then(|| AppCommand::VimKey {
            key: key.to_owned(),
            shift,
            key_char: key_char.map(str::to_owned),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_fallback_edits_in_insert_mode_and_with_vim_disabled() {
        use crate::state::AppState;
        for vim_enabled in [true, false] {
            let mut state = AppState::new();
            state.replace_vim_settings(Default::default(), vim_enabled);
            if vim_enabled {
                state.vim_enter_insert_before_cursor();
            }
            for (key, shift, text) in [
                ("a", false, Some("a")),
                ("1", true, Some("!")),
                ("e", false, Some("é")),
                ("enter", false, None),
                ("space", false, None),
                ("backspace", false, None),
                ("tab", false, None),
            ] {
                assert!(PlainInputStrategy.apply(&mut state, key, shift, text));
            }
            assert_eq!(state.active_content(), "a!é\n\t");
            PlainInputStrategy.apply(&mut state, "left", false, None);
            PlainInputStrategy.apply(&mut state, "delete", false, None);
            assert_eq!(state.active_content(), "a!é\n");
            PlainInputStrategy.apply(&mut state, "left", true, None);
            PlainInputStrategy.apply(&mut state, "x", false, Some("x"));
            assert_eq!(state.active_content(), "a!éx");
            state.undo();
            // Rapid typing may coalesce into one undo entry.
            assert_ne!(state.active_content(), "a!éx");
            state.redo();
            assert_eq!(state.active_content(), "a!éx");
            assert!(!PlainInputStrategy.apply(&mut state, "f12", false, None));
        }
    }

    #[test]
    fn printable_text_uses_the_platform_character() {
        for (key, shift, text, expected) in [
            ("1", true, Some("!"), '!'),
            ("a", true, Some("a"), 'a'),
            ("a", false, Some("A"), 'A'),
            ("e", false, Some("é"), 'é'),
            ("a", true, None, 'A'),
        ] {
            assert_eq!(
                PlainInputStrategy.command(key, shift, text),
                Some(AppCommand::InsertChar(expected))
            );
        }
    }

    #[test]
    fn vim_translation_uses_the_app_boundary() {
        assert_eq!(
            VimInputStrategy.command("d", false, Some("d")),
            Some(AppCommand::VimKey {
                key: "d".into(),
                shift: false,
                key_char: Some("d".into()),
            })
        );
        assert_eq!(
            PlainInputStrategy.command("x", false, Some("x")),
            Some(AppCommand::InsertChar('x'))
        );
    }
}
