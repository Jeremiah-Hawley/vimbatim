use super::vim::*;
use super::vim_motion::*;
use super::*;

impl AppState {
    pub fn handle_vim_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) -> bool {
        /*
         * Top-level vim key dispatcher, called by text_editor.rs for every
         * keystroke while `vim_enabled` is true and the active tab isn't in
         * Insert mode (Insert falls through to plain-editor handling by the
         * caller, except for Escape which it checks separately). Returns
         * true when the key was consumed, false when the caller should fall
         * through to its own (non-vim) handling instead.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return false;
        };
        match tab.vim_mode {
            VimMode::Normal => self.handle_vim_normal_key(key, shift, key_char),
            VimMode::Visual | VimMode::VisualLine => {
                self.handle_vim_visual_key(key, shift, key_char)
            }
            VimMode::Command => {
                self.handle_vim_command_key(key, shift, key_char);
                true
            }
            VimMode::Replace => {
                self.handle_vim_replace_key(key, shift, key_char);
                true
            }
            VimMode::Search => {
                self.handle_vim_search_key(key, shift, key_char);
                true
            }
            VimMode::Insert => false,
        }
    }

    pub(super) fn handle_vim_normal_key(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        /*
         * Normal-mode key dispatch. Routes through
         * `handle_vim_motion_key(extend: false)` first — shared with Visual
         * mode's dispatch (spec 5.6: "Motions in this mode extend the
         * selection") — see its own doc comment for the full count/
         * pending-trigger state machine and motion table. Checking it
         * *before* `:` matters: a pending `f`/`F`/`t`/`T` must still treat
         * a would-be colon keypress as its target character (state 1 of
         * the shared dispatcher), not have this function hijack it into
         * Command mode first. A `None` back means `key` isn't a motion at
         * all (and no two-keystroke command is pending it might complete)
         * — only then is `:` (not part of the shared motion system; spec
         * 5.1's table has no Visual-mode `:` transition, out of scope
         * here) and Normal's own mode-switch keys (`i`/`I`/`a`/`A`/`o`/`O`/
         * `v`/`V`) checked, with anything still unrecognized swallowed —
         * real vim's Normal mode never falls through to text insertion for
         * an unmapped key.
         *
         * Macro record start/stop (`q`, user-requested — not part of editor_instructions.md) is checked first, ahead
         * of even the motion dispatcher, but ONLY when no f/F/t/T/g
         * two-keystroke command is already pending (`vim_pending_trigger()`
         * is `None`) — otherwise `fq` (find the literal character 'q')
         * would be hijacked into starting a macro instead of completing
         * the pending find, since 'q' would never reach state 1 of
         * `handle_vim_motion_key`. `@<register>` replay is handled
         * entirely in `text_editor.rs` instead, since replaying needs to
         * re-enter GPUI-context-dependent key handling (j/k/H/M/L) that
         * this method can't reach.
         *
         * A pending `d`/`y`/`c` operator (spec 5.3) is checked before even
         * that: whichever "waiting for the next key" state is already
         * active wins, and only one can be active at a time (starting an
         * operator clears `vim_command_buf`, so a pending operator and a
         * pending find/macro-register can't coexist). Without this
         * ordering, `d` then `q` would misfire as "start recording into
         * register q" instead of correctly abandoning the pending `d`
         * (real vim: an invalid motion just cancels the operator).
         */
        // Checklist: Settings -> Vim Mode. A vim-keybind sequence already
        // in progress (`Tab.vim_keybind_seq` non-empty) claims this key
        // unconditionally, checked before even the pending operator below.
        // Safe to check first because it's mutually exclusive with every
        // other pending state in this function by construction: a sequence
        // only ever *starts* via this function's final catch-all, which is
        // only reached once every other pending state has already declined
        // the keystroke — so if the buffer is non-empty, nothing else
        // could be racing it.
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .is_some_and(|t| !t.vim_keybind_seq.is_empty())
        {
            return self.continue_vim_keybind_sequence(key, shift, key_char);
        }

        if let Some(operator) = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.vim_pending_operator)
        {
            return self.complete_vim_operator(operator, key, shift, key_char);
        }

        // `r<char>` (spec 5.5): a bare `r` arms `vim_pending_replace`, then
        // the *next* keystroke overwrites the character under the cursor
        // (or cancels harmlessly on `Escape`) rather than being interpreted
        // as anything else — checked ahead of every other pending state for
        // the same reason a pending operator is: it must claim its next key
        // unconditionally.
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.vim_pending_replace)
            .unwrap_or(false)
        {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_pending_replace = false;
            }
            if key != "escape" {
                if let Some(c) = vim_find_target_char(key, shift, key_char) {
                    self.vim_replace_char(c);
                }
            }
            return true;
        }

        // `gU`/`gu` (spec 5.3, case-change operators): a `g` is already
        // pending (from `vim_command_buf`'s ordinary `g`/`gg` trigger
        // mechanism) and this key is `u`/`U`. Checked *before*
        // `handle_vim_motion_key`, which would otherwise claim this same
        // keystroke as `gg`'s pending-completion state and simply abandon
        // it (no other `g...` command exists there yet) — starting an
        // operator instead needs to happen here, one layer up, since
        // `resolve_vim_motion`'s job is resolving motions, not starting
        // operators. `is_pending_g_case_trigger` is shared with Visual
        // mode's identical detection (`handle_vim_visual_key`); only what
        // happens *after* detecting it differs (Normal mode starts a
        // pending operator, Visual mode executes immediately). Internally
        // identified as operator `'U'`/`'u'` (not `'g'`) since they're
        // two-keystroke commands, distinguished from each other only by
        // `shift` on this second key, same pattern as every other letter
        // key in this file.
        let pending_trigger = self.vim_pending_trigger();
        if self.is_pending_g_case_trigger(pending_trigger, key) {
            self.start_vim_operator(if shift { 'U' } else { 'u' });
            return true;
        }

        if pending_trigger.is_none() {
            if self.try_handle_vim_register_prefix(key, shift, key_char) {
                return true;
            }
            if self.global_vim.vim_macro_record_pending {
                self.global_vim.vim_macro_record_pending = false;
                if let Some(register) = vim_find_target_char(key, shift, key_char) {
                    self.start_macro_recording(register);
                }
                return true;
            }
            if key == "q" && !shift {
                if self.vim_is_recording_macro() {
                    self.stop_macro_recording();
                } else {
                    self.global_vim.vim_macro_record_pending = true;
                }
                return true;
            }
        }

        if let Some(result) = self.handle_vim_motion_key(key, shift, key_char, false) {
            return result;
        }

        if matches_shifted_symbol(key, shift, key_char, ";", ":") {
            self.vim_enter_command();
            return true;
        }

        if matches_shifted_symbol(key, shift, key_char, ".", ">") {
            self.start_vim_operator('>');
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, ",", "<") {
            self.start_vim_operator('<');
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, "`", "~") {
            self.vim_toggle_case_char();
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, "/", "?") {
            self.vim_enter_search(false);
            return true;
        }
        if key == "/" || key_char == Some("/") {
            self.vim_enter_search(true);
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, "8", "*") {
            self.vim_search_word_under_cursor(true);
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, "3", "#") {
            self.vim_search_word_under_cursor(false);
            return true;
        }

        match (key, shift) {
            ("i", false) => {
                self.vim_enter_insert_before_cursor();
                true
            }
            ("i", true) => {
                self.vim_enter_insert_line_start();
                true
            }
            ("a", false) => {
                self.vim_enter_insert_after_cursor();
                true
            }
            ("a", true) => {
                self.vim_enter_insert_line_end();
                true
            }
            ("o", false) => {
                self.vim_open_line_below();
                true
            }
            ("o", true) => {
                self.vim_open_line_above();
                true
            }
            ("v", false) => {
                self.vim_enter_visual();
                true
            }
            ("v", true) => {
                self.vim_enter_visual_line();
                true
            }
            ("d", false) => {
                self.start_vim_operator('d');
                true
            }
            ("y", false) => {
                self.start_vim_operator('y');
                true
            }
            ("c", false) => {
                self.start_vim_operator('c');
                true
            }
            ("p", false) => {
                self.vim_paste_register(false);
                true
            }
            ("p", true) => {
                self.vim_paste_register(true);
                true
            }
            ("x", false) => {
                self.vim_delete_char_forward();
                true
            }
            ("x", true) => {
                self.vim_delete_char_backward();
                true
            }
            ("s", false) => {
                self.vim_substitute_char();
                true
            }
            ("s", true) => {
                self.vim_substitute_line();
                true
            }
            ("j", true) => {
                self.vim_join_lines();
                true
            }
            ("r", false) => {
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    tab.vim_pending_replace = true;
                }
                true
            }
            ("r", true) => {
                self.vim_enter_replace();
                true
            }
            ("n", false) => {
                self.vim_search_next(false);
                true
            }
            ("n", true) => {
                self.vim_search_next(true);
                true
            }
            (".", false) => {
                self.vim_repeat_last_change();
                true
            }
            // Real vim's Undo — this app's vim mode never wired it up before
            // (only `g`+`u`/`U`, the case-change operator, used the letter).
            // Calls the exact same `undo()` the app-level Undo keybind does,
            // so vim's `u` and the configurable Ctrl+Z stay in sync rather
            // than tracking two separate undo stacks. Shifted `U` (real
            // vim's "undo whole line") is out of scope, left unbound.
            ("u", false) => {
                self.undo();
                true
            }
            // Checklist: Settings -> Vim Mode. The only place a *fresh*
            // vim-keybind sequence can start — every real vim command above
            // has already had first refusal, so a key that reaches here is
            // genuinely free to be claimed. A single-key binding fires
            // immediately; a longer one starts `vim_keybind_seq` for
            // `continue_vim_keybind_sequence` (checked at the very top of
            // this function) to pick up on the next keystroke. No match:
            // silently swallowed, exactly like this catch-all always has.
            _ => {
                if let Some(c) = vim_find_target_char(key, shift, key_char) {
                    self.dispatch_fresh_vim_keybind_key(c);
                }
                true
            }
        }
    }

    /// The continuation half of the vim-keybind sequence state machine —
    /// see `handle_vim_normal_key`'s own top-of-function check, which is
    /// what routes here. `Escape`, or any key that isn't a literal
    /// character (an arrow key, say), abandons the in-progress sequence
    /// rather than silently absorbing something unrelated to it.
    pub(super) fn continue_vim_keybind_sequence(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        if key == "escape" {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_keybind_seq.clear();
            }
            return true;
        }
        let Some(c) = vim_find_target_char(key, shift, key_char) else {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_keybind_seq.clear();
            }
            return true;
        };
        let seq = {
            let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
                return true;
            };
            tab.vim_keybind_seq.push(c);
            tab.vim_keybind_seq.clone()
        };
        match self.global_vim.vim_keybinds.lookup(&seq) {
            crate::vim_keybinds::VimLookup::Exact(action) => {
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    tab.vim_keybind_seq.clear();
                }
                self.global_vim.pending_vim_action = Some(action);
            }
            crate::vim_keybinds::VimLookup::Prefix => {}
            crate::vim_keybinds::VimLookup::None => {
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    tab.vim_keybind_seq.clear();
                }
            }
        }
        true
    }

    /// A single fresh character, unclaimed by every real vim command ahead
    /// of it in `handle_vim_normal_key`'s dispatch order — checks whether it
    /// starts (or, for a one-character binding, completes) a vim-keybind
    /// sequence. Shares its match-and-branch logic with
    /// `continue_vim_keybind_sequence` but starts from an empty buffer
    /// rather than an in-progress one, so it's kept as its own small
    /// function rather than forcing one path to pretend it has a buffer to
    /// continue.
    pub(super) fn dispatch_fresh_vim_keybind_key(&mut self, c: char) {
        let seq = c.to_string();
        match self.global_vim.vim_keybinds.lookup(&seq) {
            crate::vim_keybinds::VimLookup::Exact(action) => {
                self.global_vim.pending_vim_action = Some(action);
            }
            crate::vim_keybinds::VimLookup::Prefix => {
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    tab.vim_keybind_seq = seq;
                }
            }
            crate::vim_keybinds::VimLookup::None => {}
        }
    }

    pub(super) fn handle_vim_motion_key(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
        extend: bool,
    ) -> Option<bool> {
        /*
         * Thin wrapper around `resolve_vim_motion` for Normal mode
         * (`extend = false`: a motion moves the cursor, clearing any
         * selection) and Visual/VisualLine mode (`extend = true`: the same
         * resolved target grows the active selection instead, via
         * `apply_vim_motion` -> `extend_selection` — spec 5.6).
         *
         * The one piece of `extend`-dependent routing that isn't just
         * "apply the resolved target differently": Normal mode's existing
         * `left`/`right`/`home`/`end` "let plain navigation through"
         * convenience. `resolve_vim_motion` itself always resolves these
         * locally (as h/l/0/$ equivalents — Task F's operators need that),
         * so this wrapper intercepts them *before* calling it, but only
         * when `extend` is false — letting them fall through in Visual
         * mode would corrupt the selection via the plain editor's
         * cursor-clearing Left/Right/Home/End handling, same as before
         * this method was split.
         *
         * Returns `None` when `key` isn't part of the shared motion system
         * at all — the caller (`handle_vim_normal_key`/
         * `handle_vim_visual_key`) handles those itself. Returns
         * `Some(true)` once a motion is resolved and applied, or for
         * pending-command bookkeeping. Returns `Some(false)` to signal
         * "this key needs GPUI viewport context this method doesn't have,
         * handle it in `text_editor.rs`".
         */
        if !extend && matches!(key, "left" | "right" | "home" | "end") {
            return Some(false);
        }
        match self.resolve_vim_motion(key, shift, key_char) {
            MotionResolution::NotAMotion => None,
            MotionResolution::Pending => Some(true),
            MotionResolution::NeedsGpui => Some(false),
            MotionResolution::Resolved { target, .. } => {
                Some(self.apply_vim_motion(extend, target))
            }
        }
    }

    pub(super) fn resolve_vim_motion(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> MotionResolution {
        /*
         * Shared motion resolution — the state machine every motion-aware
         * mode (Normal cursor movement, Visual/VisualLine selection
         * extension via `handle_vim_motion_key`, and Task F's `d`/`y`/`c`
         * operators, which call this directly) is built on. Resolves a
         * keystroke down to a `MotionResolution` without applying it to
         * any cursor/selection/register — application is entirely up to
         * the caller, which is *why* this exists as its own method rather
         * than being folded back into `handle_vim_motion_key`: an operator
         * needs the same target-plus-`MotionKind` a motion produces, but
         * must build a delete/yank range from it instead of moving the
         * cursor.
         *
         * A small state machine, checked in order:
         * 1. A two-keystroke command is already pending
         *    (`vim_pending_trigger()` is `Some`) — this key completes it
         *    (`gg`'s second `g`, or an `f`/`F`/`t`/`T` target character) or
         *    abandons it otherwise. Checked first so a pending find target
         *    correctly treats any key — including `;`, `g`, or a digit —
         *    as the character to search for.
         * 2. No pending command, but this key either starts/extends a
         *    `[count]` digit prefix, or starts a new two-keystroke command
         *    (`g`, `f`, `t`, or their shifted `F`/`T` forms).
         * 3. A complete, single-key motion — any count from 1/2 is
         *    consumed here. `left`/`right`/`home`/`end` are always
         *    resolved here (as h/l/0/$ equivalents) — unlike the old,
         *    single combined method, there's no Normal-mode GPUI-
         *    fallthrough special case at this layer; that's
         *    `handle_vim_motion_key`'s concern now. `up`/`down`/`j`/`k`
         *    still always need GPUI viewport context this method doesn't
         *    have, so operators can't yet act on them either (`dj`/`dk`
         *    are a documented gap, not silently wrong).
         *
         * `$`/`^`/`{`/`}` sit on shifted number/bracket keys; `key_char`,
         * the literal key itself, and the unshifted base key + `shift` are
         * all checked (`matches_shifted_symbol`) since which one GPUI
         * actually reports isn't reliable across platforms — confirmed
         * empirically after `$` didn't fire under a narrower check.
         */
        let buf = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.vim_command_buf.clone())
            .unwrap_or_default();
        let (pending_count, pending_trigger) = split_vim_command_buf(&buf);

        // 1. Complete (or abandon) a pending two-keystroke command.
        if let Some(trigger) = pending_trigger {
            self.clear_vim_command_buf();
            match trigger {
                'g' => {
                    if key == "g" && !shift {
                        let line = pending_count.unwrap_or(1);
                        if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
                            let start =
                                line_offset(&tab.document.content(), line.saturating_sub(1));
                            let target = first_nonblank(&tab.document.content(), start);
                            return MotionResolution::Resolved {
                                target,
                                kind: MotionKind::Linewise,
                            };
                        }
                    }
                    // `g$`/`g0`/`g^`: the escape hatch to real vim's original
                    // (logical-line) meaning of `$`/`0`/`^`, now that bare
                    // `$`/`0`/`^` resolve to the current *visual* row instead
                    // (`text_editor.rs`'s own interception, ahead of this
                    // dispatcher, for a heavily-wrapping document editor —
                    // see its doc comment). Deliberately the exact same
                    // `line_end`/`line_start`/`first_nonblank` calls the
                    // plain (non-`g`) arms below already use — this is only
                    // reachable from a pending `g`, so it never overlaps
                    // with them.
                    else if matches_shifted_symbol(key, shift, key_char, "4", "$") {
                        let target = self
                            .workspace
                            .tabs
                            .get(self.workspace.active_tab)
                            .map(|tab| line_end(&tab.document.content(), tab.cursor))
                            .unwrap_or(0);
                        return MotionResolution::Resolved {
                            target,
                            kind: MotionKind::InclusiveChar,
                        };
                    } else if key == "0" && !shift {
                        let target = self
                            .workspace
                            .tabs
                            .get(self.workspace.active_tab)
                            .map(|tab| line_start(&tab.document.content(), tab.cursor))
                            .unwrap_or(0);
                        return MotionResolution::Resolved {
                            target,
                            kind: MotionKind::ExclusiveChar,
                        };
                    } else if matches_shifted_symbol(key, shift, key_char, "6", "^") {
                        let target = self
                            .workspace
                            .tabs
                            .get(self.workspace.active_tab)
                            .map(|tab| first_nonblank(&tab.document.content(), tab.cursor))
                            .unwrap_or(0);
                        return MotionResolution::Resolved {
                            target,
                            kind: MotionKind::ExclusiveChar,
                        };
                    }
                    // any other key: no other `g...` command exists yet,
                    // so the sequence is simply abandoned.
                }
                'f' | 'F' | 't' | 'T' => {
                    if let Some(target_char) = vim_find_target_char(key, shift, key_char) {
                        let count = pending_count.unwrap_or(1);
                        let mut pos = self
                            .workspace
                            .tabs
                            .get(self.workspace.active_tab)
                            .map(|t| t.cursor)
                            .unwrap_or(0);
                        let mut found = false;
                        for _ in 0..count {
                            let next =
                                self.workspace
                                    .tabs
                                    .get(self.workspace.active_tab)
                                    .and_then(|t| {
                                        resolve_find(
                                            &t.document.content(),
                                            pos,
                                            trigger,
                                            target_char,
                                        )
                                    });
                            match next {
                                Some(p) => {
                                    pos = p;
                                    found = true;
                                }
                                None => break,
                            }
                        }
                        if found {
                            if let Some(tab) =
                                self.workspace.tabs.get_mut(self.workspace.active_tab)
                            {
                                tab.last_find = Some((trigger, target_char));
                            }
                            let kind = find_kind_to_motion_kind(trigger);
                            return MotionResolution::Resolved { target: pos, kind };
                        }
                    }
                }
                _ => {}
            }
            return MotionResolution::Pending;
        }

        // $/^/{/} — checked here, *before* digit-count accumulation, since
        // their unshifted base keys ("4", "6") are themselves valid count
        // digits and would otherwise be swallowed by state 2a below.
        if matches_shifted_symbol(key, shift, key_char, "4", "$") {
            self.clear_vim_command_buf();
            let target = self
                .workspace
                .tabs
                .get(self.workspace.active_tab)
                .map(|tab| line_end(&tab.document.content(), tab.cursor))
                .unwrap_or(0);
            return MotionResolution::Resolved {
                target,
                kind: MotionKind::InclusiveChar,
            };
        }
        if matches_shifted_symbol(key, shift, key_char, "6", "^") {
            self.clear_vim_command_buf();
            let target = self
                .workspace
                .tabs
                .get(self.workspace.active_tab)
                .map(|tab| first_nonblank(&tab.document.content(), tab.cursor))
                .unwrap_or(0);
            return MotionResolution::Resolved {
                target,
                kind: MotionKind::ExclusiveChar,
            };
        }
        if matches_shifted_symbol(key, shift, key_char, "[", "{") {
            let count = pending_count.unwrap_or(1);
            self.clear_vim_command_buf();
            let target = self.repeat_motion(count, paragraph_backward);
            return MotionResolution::Resolved {
                target,
                kind: MotionKind::ExclusiveChar,
            };
        }
        if matches_shifted_symbol(key, shift, key_char, "]", "}") {
            let count = pending_count.unwrap_or(1);
            self.clear_vim_command_buf();
            let target = self.repeat_motion(count, paragraph_forward);
            return MotionResolution::Resolved {
                target,
                kind: MotionKind::ExclusiveChar,
            };
        }

        // 2a. Digit count accumulation. A leading '0' is never a count
        // digit — it's the "start of line" motion (state 3) — but '0'
        // after an existing nonzero count extends it normally.
        if !shift && key.chars().count() == 1 {
            let c = key.chars().next().unwrap();
            if c.is_ascii_digit() && (c != '0' || pending_count.is_some()) {
                self.push_vim_command_buf_char(c);
                return MotionResolution::Pending;
            }
        }

        // 2b. Keys that start a new two-keystroke command.
        if key == "g" && !shift {
            self.push_vim_command_buf_char('g');
            return MotionResolution::Pending;
        }
        if key == "f" || key == "t" {
            let trigger = if shift {
                key.to_ascii_uppercase().chars().next().unwrap()
            } else {
                key.chars().next().unwrap()
            };
            self.push_vim_command_buf_char(trigger);
            return MotionResolution::Pending;
        }

        // 3. Complete, single-key motions. The count accumulated so far
        // (if any) is consumed here regardless of whether `key` turns out
        // to be recognized, so a stray count can't bleed into a later,
        // unrelated keystroke.
        let count = pending_count;
        self.clear_vim_command_buf();

        match (key, shift) {
            ("h", false) => {
                let t = self.repeat_motion(count.unwrap_or(1), char_left);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("l", false) => {
                let t = self.repeat_motion(count.unwrap_or(1), char_right);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("w", false) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_forward);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("w", true) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_forward_big);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("b", false) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_backward);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("b", true) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_backward_big);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("e", false) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_end);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::InclusiveChar,
                }
            }
            ("e", true) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_end_big);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::InclusiveChar,
                }
            }
            ("0", false) => {
                let t = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|tab| line_start(&tab.document.content(), tab.cursor))
                    .unwrap_or(0);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("_", false) => {
                let c = count.unwrap_or(1);
                let t = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|tab| underscore_motion(&tab.document.content(), tab.cursor, c))
                    .unwrap_or(0);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::Linewise,
                }
            }
            ("g", true) => {
                // `G` — no count means "last line" (sentinel usize::MAX,
                // which `line_offset`'s own clamp-on-overrun handles),
                // unlike `gg`'s "no count means line 1" above.
                let line = count.unwrap_or(usize::MAX);
                if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
                    let start = line_offset(&tab.document.content(), line.saturating_sub(1));
                    let target = first_nonblank(&tab.document.content(), start);
                    MotionResolution::Resolved {
                        target,
                        kind: MotionKind::Linewise,
                    }
                } else {
                    MotionResolution::Pending
                }
            }
            // The guard excludes the case where key_char indicates the
            // actual typed character was ':' despite shift reporting
            // false (the same GPUI-reliability concern matches_shifted_
            // symbol exists for) — falling through to None here lets the
            // caller's ':' check (which also consults key_char) claim it
            // as Command-mode entry instead of this repeat-find motion.
            (";", false) if key_char != Some(":") => match self.resolve_repeat_find(false) {
                Some((target, kind)) => MotionResolution::Resolved {
                    target,
                    kind: find_kind_to_motion_kind(kind),
                },
                None => MotionResolution::Pending,
            },
            (",", false) => match self.resolve_repeat_find(true) {
                Some((target, kind)) => MotionResolution::Resolved {
                    target,
                    kind: find_kind_to_motion_kind(kind),
                },
                None => MotionResolution::Pending,
            },
            ("left", _) => {
                let t = self.repeat_motion(1, char_left);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("right", _) => {
                let t = self.repeat_motion(1, char_right);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("home", _) => {
                let t = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|tab| line_start(&tab.document.content(), tab.cursor))
                    .unwrap_or(0);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("end", _) => {
                let t = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|tab| line_end(&tab.document.content(), tab.cursor))
                    .unwrap_or(0);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::InclusiveChar,
                }
            }
            ("up", _) | ("down", _) | ("j", false) | ("k", false) => MotionResolution::NeedsGpui,
            _ => MotionResolution::NotAMotion,
        }
    }

    pub(super) fn apply_vim_motion(&mut self, extend: bool, target: usize) -> bool {
        /*
         * The single application point every resolved motion target goes
         * through: moves the cursor and clears any selection (Normal
         * mode), or grows the active selection to `target` instead
         * (Visual/VisualLine, via the same `extend_selection` Shift+motion
         * already uses). Always returns `true` (consumed) — a thin helper
         * so every dispatch arm in `handle_vim_motion_key` can end with
         * `Some(self.apply_vim_motion(...))`.
         *
         * Also the single point that feeds the jump list (spec 5.5's
         * `Ctrl+o`/`Ctrl+i`): every Normal-mode motion lands here
         * (including `gg`/`G`, and — since `dispatch_vim_command`'s
         * `:<n>` and every search dispatch also call this — `:`-line
         * jumps and `/`/`?`/`n`/`N`/`*`/`#` too), so checking "did this
         * motion cross more than one line" once, right here, covers all
         * of `vim_todo.md`'s named "large motion" examples without
         * special-casing each call site individually. Visual-mode
         * extension (`extend`) never pushes — it's growing a selection,
         * not jumping.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            if extend {
                extend_selection(tab, target);
            } else {
                if line_index_for(&tab.document.content(), target)
                    .abs_diff(line_index_for(&tab.document.content(), tab.cursor))
                    > 1
                {
                    let old_cursor = tab.cursor;
                    tab.vim_jump_back.push(old_cursor);
                    tab.vim_jump_forward.clear();
                }
                tab.selection = None;
                tab.cursor = target;
            }
        }
        true
    }

    pub fn vim_jump_backward(&mut self) {
        /*
         * `Ctrl+o` (spec 5.5): jumps to the previous position in the jump
         * list, pushing the current position onto the forward stack so
         * `Ctrl+i` can return to it — the same back/forward-stack shape
         * as `undo`/`redo`.
         */
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        if let Some(pos) = tab.vim_jump_back.pop() {
            tab.vim_jump_forward.push(tab.cursor);
            tab.cursor = pos.min(tab.document.content().len());
            tab.selection = None;
        }
    }

    pub fn vim_jump_forward(&mut self) {
        /*
         * `Ctrl+i` (spec 5.5): the reverse of `vim_jump_backward`.
         */
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        if let Some(pos) = tab.vim_jump_forward.pop() {
            tab.vim_jump_back.push(tab.cursor);
            tab.cursor = pos.min(tab.document.content().len());
            tab.selection = None;
        }
    }

    // ── Operators: d/y/c + dd/yy/cc (spec 5.3) ───────────────────────────────────

    pub(super) fn is_pending_g_case_trigger(
        &mut self,
        pending_trigger: Option<char>,
        key: &str,
    ) -> bool {
        /*
         * Shared by `handle_vim_normal_key` and `handle_vim_visual_key`:
         * true (after clearing the pending `g`) when a `g` is pending
         * (from `vim_command_buf`'s ordinary `g`/`gg` mechanism) and `key`
         * is `u` — the detection half of `gU`/`gu` (spec 5.3). Takes the
         * caller's already-computed `pending_trigger` rather than calling
         * `vim_pending_trigger()` again. What happens *after* this returns
         * true differs by mode (Normal starts a pending operator, Visual
         * executes immediately), so only the detection is shared, not the
         * resulting action.
         */
        if pending_trigger == Some('g') && key == "u" {
            self.clear_vim_command_buf();
            true
        } else {
            false
        }
    }

    pub(super) fn try_handle_vim_register_prefix(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        /*
         * Spec 5.8's `"<register>` prefix: a bare `"` arms
         * `vim_pending_register_select`, then the *next* keystroke selects
         * which register the following `d`/`y`/`c`/`p`/`P` uses (one-shot —
         * `take_vim_selected_register` consumes it). `a`-`z` and `0` select
         * that register by name (lowercased, so shift doesn't matter);
         * `+` (shift+`=` on this keyboard layout) selects the clipboard
         * register, which `write_vim_register`/`vim_paste_register` treat
         * as just another entry in `registers` — `text_editor.rs` is the
         * only place that needs to know `'+'` is special, via the
         * `pending_clipboard_sync` mailbox. Same pattern as the existing
         * macro-register-pending flow (`vim_macro_record_pending`), and
         * checked in the same place for both reasons: it's a distinct
         * "waiting for the next key" state that must claim its key before
         * anything else (motions, operators) gets a chance to.
         */
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return false;
        };
        if tab.vim_pending_register_select {
            tab.vim_pending_register_select = false;
            if matches_shifted_symbol(key, shift, key_char, "=", "+") {
                tab.vim_selected_register = Some('+');
            } else if let Some(c) = vim_find_target_char(key, shift, key_char) {
                tab.vim_selected_register = Some(c.to_ascii_lowercase());
            }
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, "'", "\"") {
            tab.vim_pending_register_select = true;
            return true;
        }
        false
    }

    pub(super) fn take_vim_selected_register(&mut self) -> char {
        self.workspace
            .tabs
            .get_mut(self.workspace.active_tab)
            .and_then(|t| t.vim_selected_register.take())
            .unwrap_or('"')
    }

    /// The rich-clipboard metadata for `content[start..end)` — the same
    /// encoding `rich_clipboard` writes for Ctrl+C, so a vim register and the
    /// system clipboard carry formatting identically and a `"+y` can hand its
    /// formatting straight to a Ctrl+V.
    ///
    /// Must be read *before* the operator mutates anything: `d`/`c`/`x`/`s`
    /// are about to delete the very runs this describes.
    pub(super) fn vim_range_metadata(&self, start: usize, end: usize) -> String {
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return String::new();
        };
        crate::rich_clipboard::encode_with_lengths(
            &crate::document_ops::runs_in_range(tab.document.paragraphs(), start, end),
            &crate::document_ops::paragraph_attrs_in_range(tab.document.paragraphs(), start, end),
        )
    }

    pub(super) fn write_vim_register(&mut self, text: String, metadata: String, also_yank: bool) {
        /*
         * The single place any operator's removed/copied text lands in
         * `registers`: always the default (`'"'`) and, for `y`, also the
         * yank register (`'0'`) — mirroring real vim, whatever register
         * was explicitly named still updates `'"'` too. If the named
         * register was `'+'`, stages `pending_clipboard_sync` so
         * `text_editor.rs` can push it onto the real OS clipboard (needs
         * `cx`, which this file doesn't have).
         *
         * `metadata` (from `vim_range_metadata`) shadows every one of those
         * writes in `register_formats`, so `p` can put the formatting back
         * rather than letting the pasted text inherit the run it lands in.
         */
        let selected = self.take_vim_selected_register();
        self.global_vim.registers.insert('"', text.clone());
        self.global_vim
            .register_formats
            .insert('"', metadata.clone());
        if also_yank {
            self.global_vim.registers.insert('0', text.clone());
            self.global_vim
                .register_formats
                .insert('0', metadata.clone());
        }
        if selected != '"' {
            self.global_vim.registers.insert(selected, text.clone());
            self.global_vim
                .register_formats
                .insert(selected, metadata.clone());
            if selected == '+' {
                self.global_vim.pending_clipboard_sync = Some((text, metadata));
            }
        }
    }

    pub(super) fn vim_paste_register(&mut self, before: bool) {
        /*
         * `p`/`P` (spec 5.8). Reads (and consumes any `"<register>`
         * selection for) whichever register, defaulting to `'"'`.
         * Whether the paste is linewise or charwise is read off the
         * register text itself — "ends with `\n`" — rather than tracked
         * separately, since every linewise operator range already ends in
         * a trailing newline by construction (`linewise_bounds_for_operator`).
         * Linewise: inserts as a whole new line below (`p`) or above (`P`)
         * the cursor's line, landing on the pasted line's first non-blank.
         * Charwise: inserts right after (`p`) or right at (`P`) the
         * cursor, landing on the last pasted character.
         */
        let register = self.take_vim_selected_register();
        let Some(text) = self.global_vim.registers.get(&register).cloned() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        // The formatting yanked alongside the text (`write_vim_register`), in
        // the same encoding Ctrl+C uses. Absent — or unreadable, for a `+`
        // register another app filled — pastes plain, exactly as before:
        // empty runs make `sync_insert_str_with_runs` fall back to
        // `sync_insert_str`, and empty attrs leave paragraphs as the split
        // left them.
        let (runs, attrs) = self
            .global_vim
            .register_formats
            .get(&register)
            .and_then(|meta| crate::rich_clipboard::decode(meta, &text))
            .unwrap_or_default();
        let spanned = text.matches('\n').count() + 1;
        self.push_undo_snapshot();
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        if text.ends_with('\n') {
            let insert_at = if before {
                line_start(&tab.document.content(), tab.cursor)
            } else {
                let end = line_end(&tab.document.content(), tab.cursor);
                if end < tab.document.content().len() {
                    end + 1
                } else {
                    tab.document.content().len()
                }
            };
            let needs_leading_newline = insert_at == tab.document.content().len()
                && !tab.document.content().is_empty()
                && !tab.document.content().ends_with('\n');
            let first_para = tab.document.resolve_position(insert_at).0;
            // With a leading newline the paste appends a line instead of
            // splitting `first_para`, so that paragraph keeps its own
            // attributes and the new blank line at the end must not inherit
            // them; the pasted paragraphs also start one further along.
            let dest_attrs = (!needs_leading_newline)
                .then(|| {
                    tab.document
                        .paragraphs()
                        .get(first_para)
                        .map(|p| (p.heading, p.alignment))
                })
                .flatten();
            let insertion = if needs_leading_newline {
                format!("\n{}", text)
            } else {
                text
            };
            let mut runs = runs;
            if needs_leading_newline && !runs.is_empty() {
                runs.insert(
                    0,
                    Run {
                        text: "\n".to_string(),
                        ..Run::default()
                    },
                );
            }
            buffer_insert_str_with_runs(&mut tab.document, insert_at, &insertion, &runs);
            let landing_start = insert_at + if needs_leading_newline { 1 } else { 0 };
            crate::document_ops::apply_pasted_paragraph_attrs(
                tab.document.paragraphs_mut_slice(),
                first_para + usize::from(needs_leading_newline),
                spanned,
                &attrs,
                dest_attrs,
            );
            tab.cursor = first_nonblank(&tab.document.content(), landing_start);
        } else {
            let at = if before {
                tab.cursor
            } else {
                char_right(&tab.document.content(), tab.cursor)
            };
            let first_para = tab.document.resolve_position(at).0;
            let dest_attrs = tab
                .document
                .paragraphs()
                .get(first_para)
                .map(|p| (p.heading, p.alignment));
            buffer_insert_str_with_runs(&mut tab.document, at, &text, &runs);
            crate::document_ops::apply_pasted_paragraph_attrs(
                tab.document.paragraphs_mut_slice(),
                first_para,
                spanned,
                &attrs,
                dest_attrs,
            );
            let last_char_start = text.char_indices().last().map(|(i, _)| i).unwrap_or(0);
            tab.cursor = at + last_char_start;
        }
        tab.document.is_modified = true;
    }

    pub(super) fn vim_delete_char_forward(&mut self) {
        /*
         * `x` (spec 5.5): deletes the character under the cursor, writing
         * it to the register like any `d`. Clamped to the current line —
         * real vim's `x` never deletes the trailing newline (an empty
         * line, or a cursor already at the line's end, is a no-op), unlike
         * `dl`'s more general motion-based range.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let end = char_right(&tab.document.content(), tab.cursor)
            .min(line_end(&tab.document.content(), tab.cursor));
        if end == tab.cursor {
            return;
        }
        let start = tab.cursor;
        let meta = self.vim_range_metadata(start, end);
        let text = self.delete_vim_range(start, end);
        self.write_vim_register(text, meta, false);
    }

    pub(super) fn vim_delete_char_backward(&mut self) {
        /*
         * `X` (spec 5.5): deletes the character before the cursor, clamped
         * to the current line's start (a no-op at column 0).
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let start = char_left(&tab.document.content(), tab.cursor)
            .max(line_start(&tab.document.content(), tab.cursor));
        if start == tab.cursor {
            return;
        }
        let end = tab.cursor;
        let meta = self.vim_range_metadata(start, end);
        let text = self.delete_vim_range(start, end);
        self.write_vim_register(text, meta, false);
    }

    pub(super) fn vim_substitute_char(&mut self) {
        /*
         * `s` (spec 5.5): `x` immediately followed by entering Insert —
         * real vim's shorthand for "delete this one character, then type
         * its replacement".
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let end = char_right(&tab.document.content(), tab.cursor)
            .min(line_end(&tab.document.content(), tab.cursor));
        let start = tab.cursor;
        let meta = self.vim_range_metadata(start, end);
        let text = self.delete_vim_range(start, end);
        self.write_vim_register(text, meta, false);
        self.vim_enter_insert_before_cursor();
    }

    pub(super) fn vim_substitute_line(&mut self) {
        /*
         * `S` (spec 5.5): clears the current line's content (not the
         * trailing newline — same as `cc`) and enters Insert at its start.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let start = line_start(&tab.document.content(), tab.cursor);
        let end = line_end(&tab.document.content(), tab.cursor);
        let meta = self.vim_range_metadata(start, end);
        let text = self.delete_vim_range(start, end);
        self.write_vim_register(text, meta, false);
        self.vim_enter_insert_before_cursor();
    }

    pub(super) fn vim_toggle_case_char(&mut self) {
        /*
         * `~` (spec 5.5): toggles the case of the character under the
         * cursor and advances the cursor, reusing `toggle_case_vim_range`
         * (built for Visual mode's `~`). Clamped to the current line, same
         * no-op-at-EOL reasoning as `x`.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let end = char_right(&tab.document.content(), tab.cursor)
            .min(line_end(&tab.document.content(), tab.cursor));
        if end == tab.cursor {
            return;
        }
        let start = tab.cursor;
        self.toggle_case_vim_range(start, end);
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = end;
        }
    }

    pub(super) fn vim_replace_char(&mut self, replacement: char) {
        /*
         * The completion half of `r<char>` (spec 5.5): overwrites the
         * character under the cursor with `replacement` and leaves the
         * cursor in place (unlike `x`/`s`, real vim's `r` doesn't move
         * it). No-op on an empty line (nothing under the cursor to
         * replace), and — unlike every `d`/`y`/`c` operator — never
         * touches any register, matching real vim.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let end = char_right(&tab.document.content(), tab.cursor)
            .min(line_end(&tab.document.content(), tab.cursor));
        if end == tab.cursor {
            return;
        }
        let cursor = tab.cursor;
        self.replace_vim_range(cursor, end, |_| replacement.to_string());
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = cursor;
        }
    }

    pub(super) fn vim_join_lines(&mut self) {
        /*
         * `J` (spec 5.5): joins the current line with the next, replacing
         * the newline and the next line's leading spaces/tabs with a
         * single space. A no-op on the last line. Simplified vs. real
         * vim's full behavior (no special-casing for lines already ending
         * in whitespace, or a next line starting with `)`).
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let line_end_pos = line_end(&tab.document.content(), tab.cursor);
        if line_end_pos >= tab.document.content().len() {
            return;
        }
        let next_line_start = line_end_pos + 1;
        let next_line_end = line_end(&tab.document.content(), next_line_start);
        let trimmed_start = tab.document.content()[next_line_start..next_line_end]
            .char_indices()
            .find(|(_, c)| *c != ' ' && *c != '\t')
            .map(|(i, _)| next_line_start + i)
            .unwrap_or(next_line_end);
        self.replace_vim_range(line_end_pos, trimmed_start, |_| " ".to_string());
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = line_end_pos;
        }
    }

    pub(super) fn start_vim_operator(&mut self, operator: char) {
        /*
         * `d`/`y`/`c` pressed with no operator already pending: discards
         * any `[count]` sitting in `vim_command_buf` (a documented scope
         * limit — this first slice supports a count typed *after* the
         * operator, e.g. `d3w`, or between a doubled operator's two keys,
         * e.g. `d2d`, but not *before* it, e.g. `3dd`; combining both would
         * need multiplying two separate counts together, deliberately left
         * for a later pass) and marks the operator pending.
         */
        self.clear_vim_command_buf();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_pending_operator = Some(operator);
        }
        // `.` repeat (spec 5.5): starts capturing this operator's
        // completion keystrokes, unless it's `y` — yanking doesn't modify
        // the document, so it isn't a "change" `.` should repeat.
        if operator != 'y' {
            self.global_vim.vim_change_recording = Some(Vec::new());
        }
    }

    pub(super) fn clear_vim_pending_operator(&mut self) {
        /*
         * Ends a pending `d`/`y`/`c` sequence, whatever stage it was at
         * (plain, or mid-way through an `i`/`a` text-object prefix) —
         * the single place both fields are cleared together so neither
         * can be forgotten as new completion paths are added. Called on
         * *every* completion path (successful or abandoned) *before*
         * `execute_vim_operator_range` runs, so it must NOT touch
         * `vim_change_recording` — that still holds this keystroke and is
         * consumed by `execute_vim_operator_range` on success, or
         * explicitly discarded by the `NotAMotion`/`NeedsGpui` abandon
         * branch in `complete_vim_operator` on failure.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_pending_operator = None;
            tab.vim_pending_text_object_prefix = None;
        }
    }

    pub(super) fn complete_vim_operator(
        &mut self,
        operator: char,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        /*
         * Resolves the second (or third) half of an `operator[count]motion`,
         * doubled-operator (`dd`/`yy`/`cc`), or `operator[i/a]object`
         * (spec 5.4) sequence and, once resolved, executes it. Always
         * returns `true` (Normal mode swallows every keystroke while an
         * operator is pending, matching real vim rather than falling
         * through to text insertion).
         *
         * Checked in order:
         * 1. A text-object prefix (`i`/`a`) is already pending — this key
         *    names the object (`w`/`s`/`p`/a quote/a bracket char).
         *    Resolved via `vim_find_target_char` (not the raw `key`
         *    string) so shifted punctuation like `"`/`(`/`{` resolves
         *    correctly regardless of which of `key`/`key_char` GPUI
         *    happens to report it in — the same reliability concern
         *    `matches_shifted_symbol` exists for elsewhere in this file.
         * 2. The doubled-operator case (`key` matches `operator` itself),
         *    checked before delegating to `resolve_vim_motion` since
         *    `d`/`y`/`c` aren't part of the shared motion table at all —
         *    without this check the second `d` of `dd` would just resolve
         *    to `NotAMotion` and silently abandon the operator instead of
         *    running it linewise. `take_vim_count()` picks up any count
         *    typed between the two keys (`d2d`), consistent with
         *    `start_vim_operator`'s scope note.
         * 3. An `i`/`a` prefix starting a text object — also not part of
         *    the motion table, so also checked before `resolve_vim_motion`.
         * 4. Otherwise, delegate to `resolve_vim_motion`. Its `Pending`
         *    outcome (still accumulating a count or a two-keystroke motion
         *    trigger like `f`) leaves the operator pending rather than
         *    clearing it — only `Resolved`, `NeedsGpui`, and `NotAMotion`
         *    end the sequence (the latter two by abandoning it, matching
         *    real vim's "invalid motion cancels the pending operator"
         *    behaviour; `NeedsGpui` — `dj`/`dk`/`d<up>`/`d<down>` — is a
         *    documented gap, not silently wrong, since `resolve_vim_motion`
         *    has no viewport context to resolve them).
         */
        if let Some(inner) = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.vim_pending_text_object_prefix)
        {
            self.clear_vim_pending_operator();
            if let Some(object_char) = vim_find_target_char(key, shift, key_char) {
                let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                    return true;
                };
                if let Some((start, end)) =
                    resolve_vim_text_object(&tab.document.content(), tab.cursor, object_char, inner)
                {
                    self.execute_vim_operator_range(
                        operator,
                        start,
                        end,
                        MotionKind::ExclusiveChar,
                    );
                }
            }
            return true;
        }

        // The doubled-key check itself: `>`/`<` sit on shifted `.`/`,` and
        // are just as unreliable to detect via a plain string/shift
        // comparison as `$`/`^`/etc. were (same `matches_shifted_symbol`
        // reasoning) — `d`/`y`/`c` are plain unshifted letters, so the
        // simple comparison stays correct for them.
        let doubled = match operator {
            '>' => matches_shifted_symbol(key, shift, key_char, ".", ">"),
            '<' => matches_shifted_symbol(key, shift, key_char, ",", "<"),
            _ => key == operator.to_string() && !shift,
        };
        if doubled {
            let count = self.take_vim_count().unwrap_or(1);
            self.clear_vim_pending_operator();
            let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                return true;
            };
            let (start, end) =
                vim_operator_doubled_range(operator, tab.cursor, count, &tab.document.content());
            self.execute_vim_operator_range(operator, start, end, MotionKind::Linewise);
            return true;
        }

        if (key == "i" || key == "a") && !shift {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_pending_text_object_prefix = Some(key == "i");
            }
            return true;
        }

        match self.resolve_vim_motion(key, shift, key_char) {
            MotionResolution::Pending => true,
            MotionResolution::Resolved { target, kind } => {
                self.clear_vim_pending_operator();
                let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                    return true;
                };
                let (start, end) = vim_operator_motion_range(
                    operator,
                    tab.cursor,
                    target,
                    kind,
                    &tab.document.content(),
                );
                self.execute_vim_operator_range(operator, start, end, kind);
                true
            }
            MotionResolution::NeedsGpui | MotionResolution::NotAMotion => {
                self.clear_vim_pending_operator();
                // An invalid/unsupported motion abandons the operator
                // (spec 5.3) — nothing ran, so there's no change for `.`
                // to remember.
                self.global_vim.vim_change_recording = None;
                true
            }
        }
    }

    pub(super) fn execute_vim_operator_range(
        &mut self,
        operator: char,
        start: usize,
        end: usize,
        kind: MotionKind,
    ) {
        /*
         * The one place an operator's actual effect happens, given an
         * already-resolved `[start, end)` byte range (built by
         * `vim_operator_motion_range`/`vim_operator_doubled_range`, so this
         * method doesn't need to know whether it came from a motion or a
         * doubled operator). `d`/`c` write the removed text to the default
         * register (`'"'`); `y` additionally writes to `'0'`, the yank
         * register, and — unlike `d`/`c` — doesn't touch `content` at all.
         * `c` reuses `vim_enter_insert_before_cursor` for its mode
         * transition (Task D), landing in Insert at the deletion's start.
         * `>`/`<` indent/unindent every line the range spans (always
         * linewise by the time this runs — see `vim_operator_motion_range`);
         * `'U'`/`'u'` (this codebase's internal ids for `gU`/`gu`, since
         * they're two-keystroke commands, not single operator chars)
         * upper/lowercase the range's text in place.
         */
        // The range's formatting, read before `d`/`c` delete the runs it
        // describes. Only the three register-writing operators pay for it.
        let meta = matches!(operator, 'd' | 'y' | 'c')
            .then(|| self.vim_range_metadata(start, end))
            .unwrap_or_default();
        match operator {
            'd' => {
                let text = self.delete_vim_range(start, end);
                self.write_vim_register(text, meta, false);
            }
            'y' => {
                let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                    return;
                };
                let text = tab.document.content()[start..end].to_string();
                let landing = if kind == MotionKind::Linewise {
                    first_nonblank(&tab.document.content(), start)
                } else {
                    start
                };
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    tab.cursor = landing;
                    tab.selection = None;
                }
                self.write_vim_register(text, meta, true);
            }
            'c' => {
                let text = self.delete_vim_range(start, end);
                self.write_vim_register(text, meta, false);
                self.vim_enter_insert_before_cursor();
            }
            '>' => self.indent_vim_range(start, end, true),
            '<' => self.indent_vim_range(start, end, false),
            'U' => self.change_case_vim_range(start, end, true),
            'u' => self.change_case_vim_range(start, end, false),
            _ => {}
        }
        // `.` repeat (spec 5.5): commit this operator's completion
        // keystrokes now that it's actually run. `c` can't commit yet —
        // `vim_enter_insert_before_cursor` (just called above) started a
        // fresh Insert session whose typed text belongs in the same
        // change, so stash the keystrokes and let Insert's own exit
        // (`vim_exit_to_normal`) finish the commit once that text exists.
        // `y` never started a recording (see `start_vim_operator`), so
        // there's nothing to commit here for it.
        if let Some(keys) = self.global_vim.vim_change_recording.take() {
            if operator == 'c' {
                self.global_vim.vim_pending_change_before_insert = Some((operator, keys));
            } else {
                self.global_vim.last_change = Some(VimChange::Operator(operator, keys));
            }
        }
    }

    pub(super) fn replace_vim_range(
        &mut self,
        start: usize,
        end: usize,
        transform: impl FnOnce(&str) -> String,
    ) -> String {
        /*
         * Shared mutation for every operator that rewrites
         * `content[start..end]` in place (`d`/`c`'s delete, `>`/`<`'s
         * indent, `gU`/`gu`'s case-change, `~`'s case-toggle): pushes an
         * undo snapshot, replaces the range with `transform`'s output,
         * clears the selection, and marks the tab modified. Returns the
         * *original* (pre-transform) text so callers that need it
         * (delete, for registers) can use it. Deliberately doesn't set
         * the cursor — that varies by caller (delete/case-change/toggle
         * land at `start`; indent lands at the new first non-blank, which
         * needs the *post*-replace content to compute), so each caller
         * sets it themselves afterward.
         */
        self.push_undo_snapshot();
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return String::new();
        };
        let original = tab.document.content()[start..end].to_string();
        let replacement = transform(&original);
        // Every operator that rewrites a range this way (d/c/x/s/>/</gU/gu/
        // ~/r/J) gets its formatting kept in sync for free via this one
        // choke point — reduces to the same delete+insert primitives every
        // other mutation site uses.
        buffer_delete_range(&mut tab.document, start, end);
        buffer_insert_str(&mut tab.document, start, &replacement);
        tab.selection = None;
        tab.document.is_modified = true;
        original
    }

    pub(super) fn indent_vim_range(&mut self, start: usize, end: usize, indent: bool) {
        /*
         * `>`/`<`: adds or removes one leading indent unit on every line
         * `content[start..end]` spans (always whole lines by construction
         * — see `vim_operator_motion_range`). This app has no configurable
         * shiftwidth (spec doesn't define one for vim mode either), so a
         * literal tab is the indent unit, matching the plain editor's own
         * Tab-key behaviour (`text_editor.rs` inserts `'\t'`, not spaces).
         * Unindent removes one leading tab if present, else up to 4
         * leading spaces — a reasonable stand-in for "one shiftwidth" of
         * space-indented content, since there's no configured width to
         * match exactly.
         *
         * The transform rebuilds-and-splices (split on `\n`, transform
         * each line, rejoin) rather than editing in place, since
         * inserting or removing characters on an early line would
         * otherwise invalidate the byte offsets of every later line in
         * the same pass.
         */
        self.replace_vim_range(start, end, |segment| {
            let mut parts: Vec<String> = segment.split('\n').map(str::to_string).collect();
            let last = parts.len() - 1;
            for (i, line) in parts.iter_mut().enumerate() {
                if i == last && line.is_empty() {
                    // trailing empty entry from a `\n` at the very end of
                    // the segment — not a real line, leave it alone.
                    continue;
                }
                if indent {
                    line.insert(0, '\t');
                } else if line.starts_with('\t') {
                    line.remove(0);
                } else {
                    let strip = line.chars().take(4).take_while(|c| *c == ' ').count();
                    line.replace_range(0..strip, "");
                }
            }
            parts.join("\n")
        });
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = first_nonblank(&tab.document.content(), start);
        }
    }

    pub(super) fn change_case_vim_range(&mut self, start: usize, end: usize, upper: bool) {
        /*
         * `gU`/`gu`: upper/lowercases `content[start..end]` in place.
         * `String::to_uppercase`/`to_lowercase` are UTF-8-aware and may
         * change the byte length (e.g. German `ß` -> `SS`) —
         * `replace_vim_range`'s `replace_range` call handles that
         * correctly, same as every other operator mutation here.
         */
        self.replace_vim_range(start, end, |segment| {
            if upper {
                segment.to_uppercase()
            } else {
                segment.to_lowercase()
            }
        });
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = start;
        }
    }

    pub(super) fn delete_vim_range(&mut self, start: usize, end: usize) -> String {
        /*
         * The shared mutation for `d`/`c`: removes `content[start..end]`
         * and leaves the cursor at `start` — mirrors `delete_selection_
         * raw`'s undo/is_modified handling but over an explicit range
         * instead of `tab.selection`. Returns the removed text so the
         * caller can write it to a register.
         */
        let text = self.replace_vim_range(start, end, |_| String::new());
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = start;
        }
        text
    }

    pub fn vim_move_to_line_first_nonblank(&mut self, line: usize, extend: bool) {
        /*
         * Moves to (or extends the selection to, when `extend`) the first
         * non-blank character of the given 0-indexed line. Backs `H`/`M`/
         * `L` (spec 5.2), which need the live scroll position and
         * visual-row layout to know which *visual* row is currently at the
         * top/middle/bottom of the viewport — `text_editor.rs` resolves
         * that GPUI-context-dependent lookup down to a plain logical line
         * number and calls this rather than a key string, the same
         * division of labour as `j`/`k`'s `take_vim_count()`.
         */
        if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
            let start = line_offset(&tab.document.content(), line);
            let target = first_nonblank(&tab.document.content(), start);
            self.apply_vim_motion(extend, target);
        }
    }

    pub(super) fn repeat_motion(&self, count: usize, motion: fn(&str, usize) -> usize) -> usize {
        /*
         * Applies a pure single-step motion function `count` times in a
         * row, starting from the active tab's cursor, without mutating
         * anything — the caller applies the final result via
         * `apply_vim_motion`. Shared by every `[count]motion` in
         * `handle_vim_motion_key` that's a simple repeated pure function
         * (h/l/w/W/b/B/e/E/{/}); f/F/t/T need their own loop since a
         * failed search should stop the repeat early rather than clamping
         * silently.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return 0;
        };
        let mut pos = tab.cursor;
        for _ in 0..count {
            pos = motion(&tab.document.content(), pos);
        }
        pos
    }

    pub(super) fn resolve_repeat_find(&self, reverse: bool) -> Option<(usize, char)> {
        /*
         * Resolves (without applying or updating `last_find`) the target
         * for `;` (`reverse = false`) or `,` (`reverse = true`) — the
         * Visual-mode-aware counterpart to `repeat_last_find`/
         * `repeat_last_find_reverse`, sharing their nudge-past-adjacent-
         * match logic via `resolve_find_with_nudge` (always nudged: a
         * repeat is exactly when it's needed). Returns `None` when there's
         * no prior find or the repeat search itself fails to find anything
         * (both true no-ops). The returned `char` is the *effective* find
         * kind actually used (post `,`-reversal) — `f`/`F`/`t`/`T` — so
         * `resolve_vim_motion` can derive the right `MotionKind` for an
         * operator without re-deriving the reversal itself.
         */
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let (kind, target_char) = tab.last_find?;
        let kind = if reverse {
            match kind {
                'f' => 'F',
                'F' => 'f',
                't' => 'T',
                'T' => 't',
                k => k,
            }
        } else {
            kind
        };
        resolve_find_with_nudge(&tab.document.content(), tab.cursor, kind, target_char, true)
            .map(|pos| (pos, kind))
    }

    pub(super) fn handle_vim_visual_key(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        /*
         * Visual/VisualLine key dispatch. Escape and the mode-specific
         * toggle-off key (lowercase `v` closes Visual, shifted `V` closes
         * VisualLine — spec 5.1; the mismatched key/shift combination that
         * would switch directly between the two Visual variants in real
         * vim isn't in the spec table and stays out of scope, swallowed as
         * a no-op) are checked first, since they must win over everything
         * below regardless of what it would otherwise do with the same key.
         *
         * Operators (spec 5.6: `d`/`x`, `y`, `c`, `>`, `<`, `~`, `gU`,
         * `gu`) are checked next, before the shared motion dispatcher —
         * unlike Normal mode's operators, these act *immediately* on the
         * already-existing selection rather than starting a pending
         * sequence waiting for a motion (there's no "waiting for the next
         * key" state to manage here, since the selection is already
         * there). `gU`/`gu` need their own check ahead of
         * `handle_vim_motion_key` for the same reason Normal mode's does:
         * a pending `g` (from `gg`) would otherwise claim the following
         * `u`/`U` as `gg`'s failed completion and silently abandon it.
         *
         * `o` (swap which end of the selection the cursor is on) is
         * checked after operators, since it's not a motion either but also
         * isn't an operator — it doesn't touch content or exit Visual
         * mode.
         *
         * Everything else routes through `handle_vim_motion_key(extend:
         * true)` — spec 5.6: "Motions in this mode extend the selection."
         * A `None` back means `key` isn't a motion at all: unlike Normal
         * mode, this does NOT fall back to `i`/`a`/`o` mode-switch handling
         * — in Visual mode `i`/`a` are text-object prefixes (spec 5.4) for
         * a future pass (notes/editor_instructions.md §11.1 tracks this as
         * an optional, not-yet-built extension), not insert-entry.
         * Swallowed rather than falling through to text insertion, same
         * reasoning as Normal mode. `Some(false)` (the `up`/`down`/`j`/`k`
         * GPUI-context fallthrough) is propagated as-is so `text_editor.rs`
         * can apply visual-row movement with `extend: true`.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return true;
        };
        let mode = tab.vim_mode;
        if key == "escape" {
            self.vim_exit_to_normal();
            return true;
        }

        // A pending `f`/`F`/`t`/`T`/`g` trigger must win over these checks
        // — e.g. `f` then `d` must complete as "find target 'd'", not
        // misfire as starting the delete operator (and in Visual mode,
        // `f` then `v` must complete as "find target 'v'", not misfire as
        // exiting visual mode) — the same collision class the advisor
        // flagged for Normal mode's macro/operator checks, caught here by
        // this test suite's own pre-existing regression test rather than
        // shipping unverified. `gU`/`gu`'s own check
        // (`is_pending_g_case_trigger`, shared with Normal mode) is
        // narrower (only fires when `g` specifically is pending) and must
        // stay *ahead* of `handle_vim_motion_key`, which would otherwise
        // silently claim `u` as `gg`'s failed completion first.
        let pending_trigger = self.vim_pending_trigger();
        if pending_trigger.is_none() {
            match (mode, key, shift) {
                (VimMode::Visual, "v", false) => {
                    self.vim_exit_to_normal();
                    return true;
                }
                (VimMode::VisualLine, "v", true) => {
                    self.vim_exit_to_normal();
                    return true;
                }
                _ => {}
            }
        }
        if self.is_pending_g_case_trigger(pending_trigger, key) {
            self.execute_vim_visual_operator(if shift { 'U' } else { 'u' });
            return true;
        }
        if pending_trigger.is_none() {
            if self.try_handle_vim_register_prefix(key, shift, key_char) {
                return true;
            }
            if let Some(operator) = resolve_vim_visual_operator_key(key, shift, key_char) {
                self.execute_vim_visual_operator(operator);
                return true;
            }
            if key == "o" && !shift {
                self.vim_visual_swap_ends();
                return true;
            }
        }

        self.handle_vim_motion_key(key, shift, key_char, true)
            .unwrap_or(true)
    }

    pub(super) fn vim_visual_operator_range(
        &self,
        operator: char,
    ) -> Option<(usize, usize, MotionKind)> {
        /*
         * Resolves the active tab's current selection into the
         * `(start, end, MotionKind)` an operator needs — the Visual-mode
         * counterpart to `vim_operator_motion_range`, except the range is
         * already given (the selection) rather than needing to be built
         * from a cursor/target pair.
         *
         * `VisualLine` selections are always linewise; so are `>`/`<` even
         * in plain (charwise) `Visual` mode — see `operator_forces_
         * linewise`, shared with `vim_operator_motion_range`. `c` on a
         * linewise range excludes the trailing newline — see
         * `linewise_bounds_for_operator`, also shared. Recomputes the
         * line-aligned bounds from the selection's current min/max rather
         * than trusting the selection to already sit exactly on line
         * boundaries — `VisualLine`'s selection is only guaranteed
         * line-aligned at entry (`vim_enter_visual_line`); a charwise
         * motion extending it afterward isn't specially re-snapped (a
         * separate, pre-existing gap, not fixed here), so being defensive
         * about it here is what keeps *this* method correct regardless.
         */
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let (a, f) = tab.selection?;
        let (min, max) = (a.min(f), a.max(f));
        if tab.vim_mode != VimMode::VisualLine && !operator_forces_linewise(operator) {
            return Some((min, max, MotionKind::ExclusiveChar));
        }
        let last_included = if max > min { max - 1 } else { min };
        let start = line_start(&tab.document.content(), min);
        let line_end_pos = line_end(&tab.document.content(), last_included);
        let (start, end) =
            linewise_bounds_for_operator(operator, start, line_end_pos, &tab.document.content());
        Some((start, end, MotionKind::Linewise))
    }

    pub(super) fn execute_vim_visual_operator(&mut self, operator: char) {
        /*
         * Runs a Visual-mode operator (spec 5.6) against the current
         * selection and returns to Normal mode afterward — except `c`,
         * which already transitions to Insert mode on its own (via
         * `execute_vim_operator_range`'s existing `vim_enter_insert_before_
         * cursor` call, reused unchanged from Task F), so calling
         * `vim_exit_to_normal` afterward would wrongly revert that.
         * `~` (toggle case) has no Normal-mode equivalent built yet (that's
         * Task I's single-character `~`), so it gets its own small
         * `toggle_case_vim_range` rather than reusing
         * `execute_vim_operator_range`, which only knows upper/lower
         * (`gU`/`gu`), not per-character toggling.
         */
        let Some((start, end, kind)) = self.vim_visual_operator_range(operator) else {
            return;
        };
        if operator == '~' {
            self.toggle_case_vim_range(start, end);
        } else {
            self.execute_vim_operator_range(operator, start, end, kind);
        }
        if operator != 'c' {
            self.vim_exit_to_normal();
        }
    }

    pub(super) fn toggle_case_vim_range(&mut self, start: usize, end: usize) {
        /*
         * `~` in Visual mode: flips the case of every alphabetic character
         * in `content[start..end]` independently (unlike `gU`/`gu`, which
         * push everything one direction). Uses `char::to_uppercase`/
         * `to_lowercase`'s first yielded char per character rather than
         * the whole-string `String::to_uppercase`/`to_lowercase` Task F's
         * `change_case_vim_range` uses — a per-character toggle can't rely
         * on those, since each character's direction depends on its own
         * current case. A documented simplification for characters whose
         * case mapping isn't 1:1 (e.g. German `ß` -> `SS`): only the first
         * mapped character is kept.
         */
        self.replace_vim_range(start, end, |segment| {
            segment
                .chars()
                .map(|c| {
                    if c.is_uppercase() {
                        c.to_lowercase().next().unwrap_or(c)
                    } else if c.is_lowercase() {
                        c.to_uppercase().next().unwrap_or(c)
                    } else {
                        c
                    }
                })
                .collect()
        });
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = start;
        }
    }

    pub(super) fn vim_visual_swap_ends(&mut self) {
        /*
         * `o` (spec 5.6): swaps the selection's anchor and focus, moving
         * the cursor to what was previously the anchor — the highlighted
         * range itself doesn't change, only which end the cursor now sits
         * on (so a following motion extends from the *other* side).
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            if let Some((a, f)) = tab.selection {
                tab.selection = Some((f, a));
                tab.cursor = a;
            }
        }
    }

    pub(super) fn capture_vim_line_input(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> VimLineInput {
        /*
         * Shared text-capture state machine behind both Command mode
         * (`:`, spec 5.7) and Search mode (`/`/`?`, spec 5.5) — the two
         * are mutually exclusive per tab, so sharing `vim_command_line`
         * for the typed text is safe, and their `Escape`/`Enter`/
         * `Backspace`/character-capture behavior is identical; only what
         * happens with the finished text differs, which is the caller's
         * job. `Escape` discards and reports `Cancelled`. `Enter` reports
         * `Dispatch(line)` with the accumulated text (already cleared from
         * `vim_command_line`). `Backspace` deletes the last character, or
         * reports `Cancelled` if the buffer is already empty (real vim:
         * backspacing past the leading `:`/`/`/`?` cancels). Every other
         * key resolves to a literal character via `vim_find_target_char`
         * (proven correct for shifted punctuation on this GPUI backend)
         * and is appended.
         */
        if key == "escape" {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_command_line.clear();
            }
            return VimLineInput::Cancelled;
        }
        if key == "enter" {
            let line = self
                .workspace
                .tabs
                .get(self.workspace.active_tab)
                .map(|t| t.vim_command_line.clone())
                .unwrap_or_default();
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_command_line.clear();
            }
            return VimLineInput::Dispatch(line);
        }
        if key == "backspace" {
            let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
                return VimLineInput::Consumed;
            };
            if tab.vim_command_line.pop().is_none() {
                return VimLineInput::Cancelled;
            }
            return VimLineInput::Consumed;
        }
        if let Some(c) = vim_find_target_char(key, shift, key_char) {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_command_line.push(c);
            }
        }
        VimLineInput::Consumed
    }

    pub(super) fn handle_vim_command_key(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) {
        match self.capture_vim_line_input(key, shift, key_char) {
            VimLineInput::Dispatch(line) => {
                self.dispatch_vim_command(&line);
                self.vim_exit_to_normal();
            }
            VimLineInput::Cancelled => self.vim_exit_to_normal(),
            VimLineInput::Consumed => {}
        }
    }

    pub(super) fn handle_vim_search_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) {
        match self.capture_vim_line_input(key, shift, key_char) {
            VimLineInput::Dispatch(pattern) => {
                let forward = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|t| t.vim_search_direction)
                    .unwrap_or(true);
                self.dispatch_vim_search(&pattern, forward);
                self.vim_exit_to_normal();
            }
            VimLineInput::Cancelled => self.vim_exit_to_normal(),
            VimLineInput::Consumed => {}
        }
    }

    pub(super) fn dispatch_vim_command(&mut self, line: &str) {
        /*
         * Parses and executes one of spec 5.7's Command-mode commands.
         * `line` is the text typed after `:`, already stripped of the
         * leading colon by `handle_vim_command_key`. Any error (an
         * unrecognized command, or `:q` refused on unsaved changes — real
         * vim doesn't pop a confirmation dialog, it just refuses, so this
         * mirrors that instead of building new prompt UI) is recorded in
         * `vim_command_error` for the mode indicator to show; nothing here
         * ever panics or silently no-ops without saying so, except the
         * genuinely-inert `noh` (nothing to clear until Task I's search
         * highlighting exists).
         */
        let set_error = |state: &mut Self, msg: String| {
            if let Some(tab) = state.workspace.tabs.get_mut(state.workspace.active_tab) {
                tab.vim_command_error = Some(msg);
            }
        };

        match line {
            "w" => {
                if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
                    self.global_vim
                        .pending_effects
                        .push(crate::app::command::AppEffect::PerformSave(tab.id));
                }
            }
            "wa" => {
                self.global_vim.pending_effects.extend(
                    self.workspace
                        .tabs
                        .iter()
                        .map(|tab| crate::app::command::AppEffect::PerformSave(tab.id)),
                );
            }
            "q" => {
                let modified = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|t| t.document.is_modified)
                    .unwrap_or(false);
                if modified {
                    set_error(self, "E37: No write since last change".to_string());
                } else {
                    self.close_tab(self.workspace.active_tab);
                }
            }
            "q!" => self.close_tab(self.workspace.active_tab),
            "wq" | "x" => {
                if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
                    self.global_vim
                        .pending_effects
                        .push(crate::app::command::AppEffect::PerformSave(tab.id));
                }
                self.close_tab(self.workspace.active_tab);
            }
            "set vim" => self.global_vim.vim_enabled = true,
            "set novim" => self.global_vim.vim_enabled = false,
            "noh" => {} // nothing to clear yet — Task I adds search highlighting
            _ => {
                if let Some(path) = line.strip_prefix("e ") {
                    let path = self.workspace.working_directory.join(path.trim());
                    self.global_vim
                        .pending_effects
                        .push(crate::app::command::AppEffect::LoadDocument(path));
                } else if let Ok(count) = line.parse::<usize>() {
                    if count >= 1 {
                        self.vim_move_to_line_first_nonblank(count - 1, false);
                    }
                } else if let Some(rest) = line.strip_prefix("%s") {
                    if let Err(e) = self.dispatch_vim_substitute(rest) {
                        set_error(self, e);
                    }
                } else {
                    set_error(self, format!("E492: Not an editor command: {}", line));
                }
            }
        }
    }

    pub(super) fn vim_repeat_last_change(&mut self) {
        /*
         * `.` (spec 5.5): replays `last_change` at the *current* cursor
         * position. For `Operator`/`OperatorInsert`, this re-invokes
         * `start_vim_operator`/`complete_vim_operator` with the exact
         * stored completion keystrokes — since those don't need any GPUI
         * context (unlike `j`/`k`/H/M/L), the whole replay lives here in
         * `state.rs`, unlike macro replay (`@`), which needs
         * `text_editor.rs`. Re-running these also naturally re-records
         * into `vim_change_recording`/`last_change` (`start_vim_operator`/
         * `execute_vim_operator_range` don't know they're being replayed)
         * — harmless, since it just re-commits the same content.
         */
        let Some(change) = self.global_vim.last_change.clone() else {
            return;
        };
        match change {
            VimChange::Operator(operator, keys) => {
                self.start_vim_operator(operator);
                for k in &keys {
                    self.complete_vim_operator(operator, &k.key, k.shift, k.key_char.as_deref());
                }
            }
            VimChange::OperatorInsert(operator, keys, text) => {
                self.start_vim_operator(operator);
                for k in &keys {
                    self.complete_vim_operator(operator, &k.key, k.shift, k.key_char.as_deref());
                }
                self.insert_str(&text);
                self.vim_exit_to_normal();
            }
            VimChange::Insertion(text) => {
                self.insert_str(&text);
            }
        }
    }

    pub(super) fn dispatch_vim_search(&mut self, pattern: &str, forward: bool) {
        /*
         * `/pattern<Enter>` / `?pattern<Enter>` (spec 5.5). A minimal
         * `content.find`/`rfind`-with-wraparound, not a regex search — per
         * `vim_todo.md`'s explicit guidance, since a full inline find-bar
         * (highlighting, incremental search) is spec 4.6 territory and out
         * of scope here. Remembers the pattern+direction so `n`/`N` can
         * repeat it.
         */
        if pattern.is_empty() {
            return;
        }
        self.global_vim.last_search = Some((pattern.to_string(), forward));
        let cursor = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.cursor)
            .unwrap_or(0);
        self.jump_to_search_match_from(pattern, forward, cursor);
    }

    pub(super) fn jump_to_search_match_from(&mut self, pattern: &str, forward: bool, from: usize) {
        /*
         * The shared search-and-jump core: searches `pattern` starting
         * just past (`forward`) or just before (backward) `from` — never
         * matching a position the caller is already standing at — and
         * wraps around the whole document if nothing is found in that
         * direction, matching real vim's default `wrapscan` behavior.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let content = &tab.document.content();
        let found = if forward {
            let start = char_right(content, from);
            content[start..]
                .find(pattern)
                .map(|i| start + i)
                .or_else(|| content.find(pattern))
        } else {
            content[..from]
                .rfind(pattern)
                .or_else(|| content.rfind(pattern))
        };
        if let Some(pos) = found {
            self.apply_vim_motion(false, pos);
        }
    }

    pub(super) fn vim_search_next(&mut self, reverse: bool) {
        /*
         * `n`/`N` (spec 5.5): repeats the last `/`/`?`/`*`/`#` search.
         * `N` (`reverse`) searches the opposite direction from the one
         * originally used, matching real vim.
         */
        let Some((pattern, forward)) = self.global_vim.last_search.clone() else {
            return;
        };
        let effective_forward = if reverse { !forward } else { forward };
        let cursor = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.cursor)
            .unwrap_or(0);
        self.jump_to_search_match_from(&pattern, effective_forward, cursor);
    }

    pub(super) fn vim_search_word_under_cursor(&mut self, forward: bool) {
        /*
         * `*`/`#` (spec 5.5): searches for the literal word under the
         * cursor (reusing Task F's `text_object_word`), starting from
         * just past its end (`*`) or just before its start (`#`) so the
         * word the cursor is already standing in doesn't match itself.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let (start, end) = text_object_word(&tab.document.content(), tab.cursor, true);
        if start == end {
            return;
        }
        let word = tab.document.content()[start..end].to_string();
        self.global_vim.last_search = Some((word.clone(), forward));
        let from = if forward { end } else { start };
        self.jump_to_search_match_from(&word, forward, from);
    }

    pub(super) fn dispatch_vim_substitute(&mut self, rest: &str) -> Result<(), String> {
        /*
         * `:%s/pattern/replacement/[g][i]` (spec 5.7) — `rest` is
         * everything after `%s`, e.g. `/foo/bar/gi`. The delimiter is
         * always `/` (real vim allows other delimiters; out of scope
         * here). Substitutes across the whole document using the `regex`
         * crate (already a dependency). Without `g`, only the first match
         * per line is replaced, matching real vim's default.
         */
        let mut parts = rest.splitn(4, '/');
        let _ = parts.next(); // text before the first '/', always empty
        let pattern = parts.next().ok_or("E486: Pattern not found")?;
        let replacement = parts.next().ok_or("E486: Pattern not found")?;
        let flags = parts.next().unwrap_or("");
        let global = flags.contains('g');
        let case_insensitive = flags.contains('i');

        let pattern_src = if case_insensitive {
            format!("(?i){}", pattern)
        } else {
            pattern.to_string()
        };
        let re = regex::Regex::new(&pattern_src).map_err(|e| format!("E486: {}", e))?;

        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return Ok(());
        };
        let old_lines: Vec<String> = tab
            .document
            .content()
            .split('\n')
            .map(|l| l.to_string())
            .collect();
        let new_lines: Vec<String> = old_lines
            .iter()
            .map(|l| {
                if global {
                    re.replace_all(l, replacement).into_owned()
                } else {
                    re.replace(l, replacement).into_owned()
                }
            })
            .collect();
        let new_content = new_lines.join("\n");

        if new_content != tab.document.content() {
            self.push_undo_snapshot();
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                // Formatting sync scope limit (rich-text formatting plan,
                // Phase 1): a regex substitution has no clean per-character
                // mapping back to the original runs, so any paragraph whose
                // text actually changed gets replaced with a single default
                // (unformatted) run. Paragraphs the substitution didn't
                // touch keep their existing runs exactly.
                for (i, (old, new)) in old_lines.iter().zip(new_lines.iter()).enumerate() {
                    if old != new {
                        if let Some(para) = tab.document.paragraphs_mut_slice().get_mut(i) {
                            para.runs = vec![Run {
                                text: new.clone(),
                                ..Run::default()
                            }];
                        }
                    }
                }
                tab.document.is_modified = true;
                tab.cursor = tab.cursor.min(tab.document.content().len());
                tab.selection = None;
            }
        }
        Ok(())
    }

    pub(super) fn handle_vim_replace_key(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) {
        /*
         * `R` mode (spec 5.5, `VimMode::Replace`). `Escape` returns to
         * Normal. `Backspace` moves the cursor back one character —
         * deliberately not restoring whatever it overwrote (real vim
         * tracks per-position originals so backspacing is non-destructive;
         * out of scope here, documented in `vim_todo.md`). Anything else
         * resolves to a literal character via `vim_find_target_char` (same
         * resolver as Command mode's text capture) and overwrites in place.
         */
        if key == "escape" {
            self.vim_exit_to_normal();
            return;
        }
        if key == "backspace" {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.cursor = char_left(&tab.document.content(), tab.cursor);
            }
            return;
        }
        if let Some(c) = vim_find_target_char(key, shift, key_char) {
            self.vim_replace_mode_type_char(c);
        }
    }

    pub(super) fn vim_replace_mode_type_char(&mut self, c: char) {
        /*
         * Overwrites the character under the cursor with `c` and advances
         * past it — or, once the cursor reaches the end of the line (or
         * document), appends instead, since there's nothing left to
         * overwrite (matches real vim: Replace mode can extend a line's
         * length by typing past its original end).
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        if tab.cursor < line_end(&tab.document.content(), tab.cursor) {
            let end = char_right(&tab.document.content(), tab.cursor);
            let cursor = tab.cursor;
            self.replace_vim_range(cursor, end, |_| c.to_string());
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.cursor = cursor + c.len_utf8();
            }
        } else {
            self.insert_char(c);
        }
    }
}
