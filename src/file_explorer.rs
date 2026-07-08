use gpui::prelude::*;
use gpui::*;

use std::path::PathBuf;

use crate::docx_parser::create_new_docx;
use crate::state::{default_paragraphs, AppState, FileNode, SidebarMode};
use crate::theme::{palette, radius, space, Palette};

/// The collapsible file explorer sidebar shown on the right side of the window.
///
/// Has two modes (`AppState.sidebar_mode`), toggled by the Files/Nav button
/// pair in its own header or the ribbon's Nav button:
///   • Files (default): a recursive tree of directories and .docx files
///     rooted at the working directory. Double-clicking a file opens it in
///     a new tab.
///   • Nav: an outline of the active tab's Pocket/Hat/Block/Tag headings,
///     nested by type. Clicking one jumps the editor to that line.
pub struct FileExplorer {
    state: Entity<AppState>,
}

impl FileExplorer {
    pub fn new(state: Entity<AppState>) -> Self {
        /*
         * Constructs the FileExplorer. File tree data lives in AppState so that
         * the rest of the app can react to file changes without querying the
         * sidebar directly.
         */
        FileExplorer { state }
    }

    fn create_new_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Creates a new blank .docx file in the working directory. Picks the
         * first unused name in the "Untitled.docx", "Untitled 1.docx", … series.
         *
         * In a future iteration this should open a modal asking for a custom name
         * rather than auto-generating one.
         */
        let dir = self.state.read(cx).working_directory.clone();

        let mut name = "Untitled.docx".to_string();
        let mut counter = 1;
        // Find the first available filename in the sequence
        while dir.join(&name).exists() {
            name = format!("Untitled {}.docx", counter);
            counter += 1;
        }
        let path = dir.join(&name);

        // Write a valid minimal .docx so the file can be parsed and saved immediately.
        if let Err(e) = create_new_docx(&default_paragraphs(), &path) {
            eprintln!("[FileExplorer] failed to create {}: {}", path.display(), e);
            return;
        }

        self.state.update(cx, |s, cx| {
            s.refresh_file_tree();
            s.open_file(path);
            cx.notify();
        });
        cx.notify();
    }

    /// Recursively renders a single FileNode and (if expanded) its children.
    ///
    /// Returns `AnyElement` so the two arms of the match can have different
    /// concrete element types (`Div` vs `Stateful<Div>`) while still unifying
    /// into a single return type for Rust's type checker.
    fn render_node(
        node: &FileNode,
        depth: usize,
        active_path: &Option<PathBuf>,
        p: Palette,
        state_handle: &Entity<AppState>,
        cx: &mut Context<FileExplorer>,
    ) -> AnyElement {
        /*
         * Renders one row in the tree:
         *   • Directory → shows a chevron that toggles expand/collapse on click
         *   • File      → opens the file on double-click (click_count == 2)
         *
         * Indentation is achieved with left-padding proportional to `depth`.
         * IDs are derived from the filesystem path so they are stable across renders.
         */
        let indent = px((depth * 16) as f32);

        match node {
            FileNode::Dir {
                name,
                path,
                children,
                expanded,
            } => {
                let chevron = if *expanded { "▾ " } else { "▸ " };
                let path_clone = path.clone();
                let state_clone = state_handle.clone();
                let children_snap = children.clone();
                let is_expanded = *expanded;
                let dir_name = name.clone();
                // Use path string as a unique element ID for this directory
                let dir_row_id = ElementId::from(path.to_string_lossy().into_owned());

                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(dir_row_id)
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(24.0))
                            .pl(indent)
                            .pr(px(space::SM))
                            .cursor_pointer()
                            .text_sm()
                            .text_color(rgb(p.text))
                            .border_l_2()
                            .border_color(rgb(p.sidebar))
                            .hover(move |s| s.bg(rgb(p.chrome_hover)))
                            .active(move |s| s.bg(rgb(p.chrome_active)))
                            .on_click(move |_ev, _window, cx| {
                                let p = path_clone.clone();
                                state_clone.update(cx, |s, cx| {
                                    toggle_dir_expanded(&mut s.file_tree, &p);
                                    cx.notify();
                                });
                            })
                            .child(div().text_color(rgb(p.text_muted)).child(chevron))
                            .child(div().child(dir_name)),
                    )
                    // Recursively render children when the directory is expanded
                    .when(is_expanded, |d| {
                        d.children(children_snap.iter().map(|child| {
                            Self::render_node(child, depth + 1, active_path, p, state_handle, cx)
                        }))
                    })
                    .into_any_element()
            }

            FileNode::File { name, path } => {
                let is_active = active_path.as_ref().is_some_and(|active| active == path);
                let path_clone = path.clone();
                let state_clone = state_handle.clone();
                let file_name = name.clone();
                // Use path string as a unique element ID for this file row
                let file_id = ElementId::from(path.to_string_lossy().into_owned());

                div()
                    .id(file_id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(24.0))
                    .pl(indent)
                    .pr(px(space::SM))
                    .cursor_pointer()
                    .text_sm()
                    .text_color(if is_active {
                        rgb(p.text)
                    } else {
                        rgb(p.text_muted)
                    })
                    .bg(if is_active {
                        rgb(p.selection)
                    } else {
                        rgb(p.sidebar)
                    })
                    .border_l_2()
                    .border_color(if is_active {
                        rgb(p.accent)
                    } else {
                        rgb(p.sidebar)
                    })
                    .hover(move |s| s.bg(rgb(p.chrome_hover)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    // Open on double-click; single click is for selection (future)
                    .on_click(move |ev, _window, cx| {
                        if ev.click_count() >= 2 {
                            let p: PathBuf = path_clone.clone();
                            state_clone.update(cx, |s, cx| {
                                s.open_file(p);
                                cx.notify();
                            });
                        }
                    })
                    .child(
                        div()
                            .w(px(28.0))
                            .text_xs()
                            .text_color(if is_active {
                                rgb(p.text)
                            } else {
                                rgb(p.text_faint)
                            })
                            .child("DOC"),
                    )
                    .child(div().child(file_name))
                    .into_any_element()
            }
        }
    }

    /// One half of the Files/Nav header toggle: highlighted when it
    /// matches the sidebar's current mode, click sets `AppState.sidebar_mode`
    /// to `mode` otherwise. A plain closure (not `cx.listener`) since it
    /// only ever needs `state_handle`, never `FileExplorer`'s own fields —
    /// same pattern `render_node`'s click handlers already use.
    fn render_mode_toggle_btn(
        id: &'static str,
        label: &'static str,
        mode: SidebarMode,
        current: SidebarMode,
        p: Palette,
        state_handle: &Entity<AppState>,
    ) -> impl IntoElement {
        let is_active = mode == current;
        let state_clone = state_handle.clone();
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .h(px(24.0))
            .px(px(space::SM))
            .rounded(px(radius::MD))
            .cursor_pointer()
            .text_xs()
            .border_1()
            .when(is_active, |d| {
                d.bg(rgb(p.accent_wash)).text_color(rgb(p.text)).border_color(rgb(p.accent_muted))
            })
            .when(!is_active, |d| {
                d.text_color(rgb(p.text_muted))
                    .border_color(rgb(p.border_subtle))
                    .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
            })
            .on_click(move |_ev, _window, cx| {
                state_clone.update(cx, |s, cx| {
                    s.sidebar_mode = mode;
                    cx.notify();
                });
            })
            .child(label)
    }

    /// Renders the Nav mode's heading outline: every line in the active
    /// tab whose `Paragraph.heading` is 1–4 (Pocket/Hat/Block/Tag — set by
    /// `AppState::apply_card_style`), in document order, indented by type
    /// (Pocket flush left, Tag deepest). Clicking a row jumps the editor to
    /// that line via `AppState::jump_to_line`. Shows a placeholder message
    /// instead of an empty list when the active tab has no headings yet.
    fn render_nav_tree(state_handle: &Entity<AppState>, p: Palette, cx: &mut Context<FileExplorer>) -> AnyElement {
        let state = state_handle.read(cx);
        let Some(tab) = state.tabs.get(state.active_tab) else {
            return div().into_any_element();
        };

        // content and paragraphs are always kept 1:1 (one paragraph per
        // line) — the same pairing wikifi_export.rs's own heading walk uses.
        let headings: Vec<(usize, u8, String)> = tab
            .content
            .split('\n')
            .enumerate()
            .filter_map(|(line_idx, line_text)| {
                let heading = tab.paragraphs.get(line_idx)?.heading;
                (1..=4).contains(&heading).then(|| (line_idx, heading, line_text.to_string()))
            })
            .collect();

        if headings.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p(px(space::MD))
                .text_sm()
                .text_color(rgb(p.text_faint))
                .child("No headings yet")
                .into_any_element();
        }

        div()
            .id("nav-scroll")
            .flex_1()
            .overflow_y_scroll()
            .py(px(space::XS))
            .children(headings.into_iter().map(|(line_idx, heading, text)| {
                let indent = px(((heading - 1) as f32) * 16.0);
                let row_id = ElementId::named_usize("nav-heading", line_idx);
                let state_clone = state_handle.clone();

                div()
                    .id(row_id)
                    .flex()
                    .items_center()
                    .h(px(24.0))
                    .pl(indent + px(space::MD))
                    .pr(px(space::SM))
                    .cursor_pointer()
                    .text_sm()
                    .text_color(rgb(p.text))
                    .truncate()
                    .hover(move |s| s.bg(rgb(p.chrome_hover)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    .on_click(move |_ev, _window, cx| {
                        state_clone.update(cx, |s, cx| {
                            s.jump_to_line(line_idx);
                            cx.notify();
                        });
                    })
                    .child(text)
            }))
            .into_any_element()
    }
}

impl Render for FileExplorer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the sidebar as a fixed-width 240px panel with two modes
         * (AppState.sidebar_mode), both toggled by the same Files/Nav
         * button pair in the header (and mirrored by the ribbon's own Nav
         * button, formatting_ribbon.rs):
         *   • Files: working-directory name + refresh/+ buttons, body is
         *     the scrollable file tree built by render_node().
         *   • Nav:   active tab's title, no refresh/+ (not applicable),
         *     body is the heading outline built by render_nav_tree().
         *
         * The scroll container requires `.id()` before `.overflow_y_scroll()` because
         * scroll state is tracked per-element-ID in GPUI.
         */
        let state = self.state.read(cx);
        let p = palette(state.theme);
        let sidebar_mode = state.sidebar_mode;
        let dir_name = state
            .working_directory
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".")
            .to_string();
        let active_tab_title = state.tabs.get(state.active_tab).map(|t| t.title.clone());
        let file_tree = state.file_tree.clone();
        let active_path = state
            .tabs
            .get(state.active_tab)
            .and_then(|tab| tab.file_path.clone());
        let _ = state;

        let state_handle = self.state.clone();

        div()
            .flex()
            .flex_col()
            .w(px(240.0))
            .h_full()
            .bg(rgb(p.sidebar))
            .border_r_1()
            .border_color(rgb(p.border))
            // ── Header ────────────────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(44.0))
                    .px(px(space::MD))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::XXS))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(p.text))
                                    .font_weight(FontWeight::BOLD)
                                    .child(match sidebar_mode {
                                        SidebarMode::Files => "Files",
                                        SidebarMode::Nav => "Navigation",
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(p.text_faint))
                                    .child(match sidebar_mode {
                                        SidebarMode::Files => dir_name,
                                        SidebarMode::Nav => active_tab_title.unwrap_or_default(),
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(space::XS))
                            // Files/Nav toggle — switches this whole panel between the
                            // file tree and the heading outline. The ribbon's own Nav
                            // button (formatting_ribbon.rs) flips the exact same
                            // AppState.sidebar_mode field.
                            .child(Self::render_mode_toggle_btn(
                                "files-mode-btn", "Files", SidebarMode::Files, sidebar_mode, p, &state_handle,
                            ))
                            .child(Self::render_mode_toggle_btn(
                                "nav-mode-btn", "Nav", SidebarMode::Nav, sidebar_mode, p, &state_handle,
                            ))
                            .when(sidebar_mode == SidebarMode::Files, |d| {
                                d
                                    // Refresh button — re-scans the working directory so
                                    // files created in external applications become
                                    // visible without restarting vimbatim.
                                    .child(
                                        div()
                                            .id("refresh-file-btn")
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .w(px(26.0))
                                            .h(px(24.0))
                                            .rounded(px(radius::MD))
                                            .cursor_pointer()
                                            .text_color(rgb(p.text_muted))
                                            .text_sm()
                                            .border_1()
                                            .border_color(rgb(p.border_subtle))
                                            .hover(move |s| {
                                                s.bg(rgb(p.chrome_hover))
                                                    .text_color(rgb(p.text))
                                                    .border_color(rgb(p.border))
                                            })
                                            .active(move |s| s.bg(rgb(p.chrome_active)))
                                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                                this.state.update(cx, |s, cx| {
                                                    s.refresh_file_tree();
                                                    cx.notify();
                                                });
                                                cx.notify();
                                            }))
                                            .child("↻"),
                                    )
                                    .child(
                                        div()
                                            .id("new-file-btn")
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .w(px(26.0))
                                            .h(px(24.0))
                                            .rounded(px(radius::MD))
                                            .cursor_pointer()
                                            .text_color(rgb(p.text_muted))
                                            .text_sm()
                                            .border_1()
                                            .border_color(rgb(p.border_subtle))
                                            .hover(move |s| {
                                                s.bg(rgb(p.chrome_hover))
                                                    .text_color(rgb(p.text))
                                                    .border_color(rgb(p.border))
                                            })
                                            .active(move |s| s.bg(rgb(p.chrome_active)))
                                            .on_click(cx.listener(|this, _ev, window, cx| {
                                                this.create_new_file(window, cx);
                                            }))
                                            .child("+"),
                                    )
                            }),
                    ),
            )
            // ── Body: file tree or heading outline, depending on sidebar_mode ──
            // `.id()` must come before `.overflow_y_scroll()` because GPUI tracks
            // scroll position per unique element ID.
            .child(match sidebar_mode {
                SidebarMode::Files => div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .py(px(space::XS))
                    .children(file_tree.iter().map(|node| {
                        Self::render_node(node, 0, &active_path, p, &state_handle, cx)
                    }))
                    .into_any_element(),
                SidebarMode::Nav => Self::render_nav_tree(&state_handle, p, cx),
            })
    }
}

/// Recursively searches the mutable tree for a `FileNode::Dir` whose path matches
/// `target` and flips its `expanded` flag.
fn toggle_dir_expanded(tree: &mut Vec<FileNode>, target: &PathBuf) {
    /*
     * Walks `tree` in-place. On finding a matching directory, it flips `expanded`
     * and returns early. Children are searched recursively before returning.
     */
    for node in tree.iter_mut() {
        if let FileNode::Dir {
            path,
            expanded,
            children,
            ..
        } = node
        {
            if path == target {
                *expanded = !*expanded;
                return;
            }
            toggle_dir_expanded(children, target);
        }
    }
}
