//! GPUI-free editor key translation.

use crate::app::command::AppCommand;

/// Translates an editor keystroke into an application command when the active
/// editing mode owns it. Platform focus, scrolling, and clipboard work stay in
/// the view adapter.
pub(crate) trait InputStrategy {
    fn command(&self, key: &str, shift: bool, key_char: Option<&str>) -> Option<AppCommand>;
}

/// Plain text editing is handled by GPUI's text/view adapter; it has no
/// application command for an individual character.
pub(crate) struct PlainInputStrategy;

impl InputStrategy for PlainInputStrategy {
    fn command(&self, _: &str, _: bool, _: Option<&str>) -> Option<AppCommand> {
        None
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
    fn vim_translation_uses_the_app_boundary() {
        assert_eq!(
            VimInputStrategy.command("d", false, Some("d")),
            Some(AppCommand::VimKey {
                key: "d".into(),
                shift: false,
                key_char: Some("d".into()),
            })
        );
        assert_eq!(PlainInputStrategy.command("x", false, Some("x")), None);
    }
}
