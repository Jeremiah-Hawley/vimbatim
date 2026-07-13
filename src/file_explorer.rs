use gpui::prelude::*;
use gpui::*;

use std::collections::HashSet;
use std::path::PathBuf;

use crate::state::{AppState, FileContextMenu, FileContextMenuTarget, FileNode, SidebarMode};
use crate::theme::{palette, radius, space, Palette};

/// Drag payload used solely to identify a sidebar-resize drag to
/// `MainWindow`'s `on_drag_move` handler (`main_window.rs`) — GPUI
/// discriminates `on_drag_move` listeners by payload type, so this carries
/// no data of its own (the live mouse position comes from the drag event
/// itself). Must implement `Render` because GPUI uses the payload as the
/// ghost view that would follow the cursor while dragging; an invisible
/// `gpui::Empty` is correct here since the resize is felt through the
/// sidebar's own width changing live, not a floating ghost — mirrors
/// `workspace::DraggedDock` in Zed's own dock-resize implementation, the
/// reference this was built from.
#[derive(Clone)]
pub struct SidebarResizePayload;

impl Render for SidebarResizePayload {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// The collapsible file explorer sidebar shown on the left side of the window.
///
/// Has two modes (`AppState.sidebar_mode`), toggled by the Files/Nav button
/// pair in its own header or the ribbon's Nav button:
///   • Files (default): a recursive tree of directories and .docx files
///     rooted at the working directory. Double-clicking a file opens it in
///     a new tab.
///   • Nav: an outline of the active tab's Pocket/Hat/Block/Tag headings,
///     nested by type. Clicking one jumps the editor to that line.
///
/// Width is `AppState.sidebar_width`, changeable by dragging the resize
/// handle on its right edge (`render`, bottom).
pub struct FileExplorer {
    state: Entity<AppState>,
    /// Line indices (into the active tab's content) of Nav headings the
    /// user has collapsed, hiding their nested headings. View-only UI
    /// state — unlike the file tree's own expand/collapse (`FileNode::Dir.
    /// expanded`, stored in `AppState` since file-tree structure is itself
    /// shared app state), nothing else needs to know or persist this, so it
    /// lives here rather than being threaded through `AppState`. Cleared
    /// implicitly by nothing — collapsed state for a line index survives
    /// edits elsewhere in the document, same as the file tree's `expanded`
    /// flags survive unrelated file operations.
    nav_collapsed: HashSet<usize>,
}

impl FileExplorer {
    pub fn new(state: Entity<AppState>) -> Self {
        /*
         * Constructs the FileExplorer. File tree data lives in AppState so that
         * the rest of the app can react to file changes without querying the
         * sidebar directly.
         */
        FileExplorer { state, nav_collapsed: HashSet::new() }
    }

    fn create_new_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Creates a new blank .docx file in the working directory via
         * `AppState::create_new_docx_in`, shared with the right-click
         * menu's "New File" (which targets wherever was clicked instead of
         * always the tree's root).
         *
         * In a future iteration this should open a modal asking for a custom name
         * rather than auto-generating one.
         */
        let dir = self.state.read(cx).working_directory.clone();
        self.state.update(cx, |s, cx| {
            if let Err(e) = s.create_new_docx_in(&dir) {
                eprintln!("[FileExplorer] failed to create new file in {}: {}", dir.display(), e);
            }
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
                let path_for_ctx = path.clone();
                let state_for_ctx = state_handle.clone();
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
                            .min_h(px(24.0))
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
                            // Right-click: open the context menu targeting
                            // this directory ("New File" creates inside it).
                            // stop_propagation so the sidebar body's own
                            // Background right-click handler doesn't also fire.
                            .on_mouse_down(MouseButton::Right, move |ev, _window, cx| {
                                cx.stop_propagation();
                                let position = (ev.position.x.as_f32(), ev.position.y.as_f32());
                                let target = FileContextMenuTarget::Dir(path_for_ctx.clone());
                                state_for_ctx.update(cx, |s, cx| {
                                    s.open_file_context_menu(position, target);
                                    cx.notify();
                                });
                            })
                            .child(div().text_color(rgb(p.text_muted)).child(chevron))
                            .child(div().flex_1().min_w_0().line_clamp(2).child(dir_name)),
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
                let path_for_ctx = path.clone();
                let state_for_ctx = state_handle.clone();
                let file_name = name.clone();
                // Use path string as a unique element ID for this file row
                let file_id = ElementId::from(path.to_string_lossy().into_owned());

                div()
                    .id(file_id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_h(px(24.0))
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
                    // Right-click: open the context menu targeting this
                    // file ("Delete" acts on it; "New File" creates
                    // alongside it, in its parent directory).
                    // stop_propagation so the sidebar body's own Background
                    // right-click handler doesn't also fire.
                    .on_mouse_down(MouseButton::Right, move |ev, _window, cx| {
                        cx.stop_propagation();
                        let position = (ev.position.x.as_f32(), ev.position.y.as_f32());
                        let target = FileContextMenuTarget::File(path_for_ctx.clone());
                        state_for_ctx.update(cx, |s, cx| {
                            s.open_file_context_menu(position, target);
                            cx.notify();
                        });
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
                    .child(div().flex_1().min_w_0().line_clamp(2).child(file_name))
                    .into_any_element()
            }
        }
    }

    /// Renders the file explorer's right-click menu (`AppState.file_context_menu`,
    /// found_bugs.md's Forgotten Implicit Feature) as a floating panel pinned
    /// to the click position via GPUI's `anchored()` (window-relative
    /// coordinates, matching `MouseDownEvent.position`) wrapped in
    /// `deferred()` so it paints above the file tree regardless of where it
    /// sits in the element tree — the same two primitives Zed's own
    /// context menus are built on.
    ///
    /// Two states: the normal menu ("New File", plus "Delete" only for a
    /// File target — deleting a directory needs stronger confirmation than
    /// this menu offers, so it's not shown for Dir/Background), and a
    /// "Delete <name>? Confirm / Cancel" step once "Delete" has been
    /// clicked once (`FileContextMenu.confirming_delete`) — a real
    /// filesystem delete has no undo, so it isn't one click.
    fn render_context_menu(
        menu: FileContextMenu,
        p: Palette,
        state_handle: &Entity<AppState>,
        _cx: &mut Context<FileExplorer>,
    ) -> AnyElement {
        let (x, y) = menu.position;

        let panel = if menu.confirming_delete {
            let display_name = match &menu.target {
                FileContextMenuTarget::File(path) => {
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("this file").to_string()
                }
                _ => "this file".to_string(),
            };
            let cancel_state = state_handle.clone();
            let confirm_state = state_handle.clone();

            div()
                .flex()
                .flex_col()
                .gap(px(space::XS))
                .w(px(220.0))
                .bg(rgb(p.chrome))
                .border_1()
                .border_color(rgb(p.border))
                .rounded(px(radius::MD))
                .shadow_lg()
                .p(px(space::SM))
                .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(p.text))
                        .child(format!("Delete \"{}\"? This can't be undone.", display_name)),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(space::XS))
                        .child(
                            div()
                                .id("ctx-menu-cancel")
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .h(px(26.0))
                                .rounded(px(radius::MD))
                                .cursor_pointer()
                                .text_sm()
                                .text_color(rgb(p.text_muted))
                                .border_1()
                                .border_color(rgb(p.border_subtle))
                                .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                                .on_click(move |_ev, _window, cx| {
                                    cancel_state.update(cx, |s, cx| {
                                        s.close_file_context_menu();
                                        cx.notify();
                                    });
                                })
                                .child("Cancel"),
                        )
                        .child(
                            div()
                                .id("ctx-menu-confirm-delete")
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .h(px(26.0))
                                .rounded(px(radius::MD))
                                .cursor_pointer()
                                .text_sm()
                                .text_color(rgb(0xf14c4c))
                                .border_1()
                                .border_color(rgb(0xf14c4c))
                                .hover(move |s| s.bg(rgba(0xf14c4c33)))
                                .on_click(move |_ev, _window, cx| {
                                    confirm_state.update(cx, |s, cx| {
                                        if let Err(e) = s.confirm_context_menu_delete() {
                                            eprintln!("[FileExplorer] failed to delete: {}", e);
                                        }
                                        cx.notify();
                                    });
                                })
                                .child("Delete"),
                        ),
                )
                .into_any_element()
        } else {
            let can_delete = matches!(menu.target, FileContextMenuTarget::File(_));
            let new_file_state = state_handle.clone();
            let delete_state = state_handle.clone();

            div()
                .flex()
                .flex_col()
                .w(px(160.0))
                .bg(rgb(p.chrome))
                .border_1()
                .border_color(rgb(p.border))
                .rounded(px(radius::MD))
                .shadow_lg()
                .py(px(space::XXS))
                .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("ctx-menu-new-file")
                        .h(px(26.0))
                        .px(px(space::SM))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(rgb(p.text))
                        .hover(move |s| s.bg(rgb(p.chrome_hover)))
                        .on_click(move |_ev, _window, cx| {
                            new_file_state.update(cx, |s, cx| {
                                if let Err(e) = s.create_file_at_context_menu_location() {
                                    eprintln!("[FileExplorer] failed to create file: {}", e);
                                }
                                cx.notify();
                            });
                        })
                        .child("New File"),
                )
                .when(can_delete, |d| {
                    d.child(
                        div()
                            .id("ctx-menu-delete")
                            .h(px(26.0))
                            .px(px(space::SM))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .text_sm()
                            .text_color(rgb(0xf14c4c))
                            .hover(move |s| s.bg(rgba(0xf14c4c33)))
                            .on_click(move |_ev, _window, cx| {
                                delete_state.update(cx, |s, cx| {
                                    s.request_context_menu_delete_confirmation();
                                    cx.notify();
                                });
                            })
                            .child("Delete"),
                    )
                })
                .into_any_element()
        };

        deferred(anchored().position(point(px(x), px(y))).snap_to_window().child(panel))
            .with_priority(1)
            .into_any_element()
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

    /// Renders the Nav mode's heading outline: every line in the active tab
    /// whose `Paragraph.heading` is 1–4 (Pocket/Hat/Block/Tag — set by
    /// `AppState::apply_card_style`), nested by actual document structure
    /// (see `build_nav_entries`) rather than raw heading level, so a
    /// heading's collapse arrow hides exactly the headings between it and
    /// its next sibling — not an unrelated, fixed set of "everything at a
    /// deeper type". Clicking a row's text jumps the editor to that line
    /// via `AppState::jump_to_line`; clicking its arrow (present only when
    /// it has nested headings) toggles `self.nav_collapsed` instead. Shows
    /// a placeholder message instead of an empty list when the active tab
    /// has no headings yet.
    fn render_nav_tree(&self, state_handle: &Entity<AppState>, p: Palette, cx: &mut Context<FileExplorer>) -> AnyElement {
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

        let entries = build_nav_entries(&headings, &self.nav_collapsed);

        div()
            .id("nav-scroll")
            .flex_1()
            .overflow_y_scroll()
            .py(px(space::XS))
            .children(entries.into_iter().map(|entry| {
                let indent = px((entry.depth as f32) * 16.0);
                let line_idx = entry.line_idx;
                let is_collapsed = self.nav_collapsed.contains(&line_idx);
                let state_clone = state_handle.clone();

                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(24.0))
                    .pl(indent + px(space::SM))
                    .pr(px(space::SM))
                    // Arrow: only present when this heading has nested
                    // headings to collapse. A fixed-width spacer otherwise,
                    // so leaf headings' text still lines up with siblings
                    // that do have one.
                    .child(if entry.has_children {
                        let arrow_id = ElementId::named_usize("nav-arrow", line_idx);
                        div()
                            .id(arrow_id)
                            .w(px(16.0))
                            .flex_shrink_0()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(p.text_muted))
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                if !this.nav_collapsed.remove(&line_idx) {
                                    this.nav_collapsed.insert(line_idx);
                                }
                                cx.notify();
                            }))
                            .child(if is_collapsed { "▸" } else { "▾" })
                            .into_any_element()
                    } else {
                        div().w(px(16.0)).flex_shrink_0().into_any_element()
                    })
                    .child(
                        div()
                            .id(ElementId::named_usize("nav-heading", line_idx))
                            .flex_1()
                            .min_w_0()
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
                            .child(entry.text),
                    )
            }))
            .into_any_element()
    }
}

/// One row in the Nav tree, already resolved to its rendering position —
/// see `build_nav_entries`.
struct NavEntry {
    line_idx: usize,
    text: String,
    /// Nesting depth from actual document structure (0 = no ancestor
    /// heading), NOT the raw `heading` level — a Tag with no preceding
    /// Pocket/Hat/Block ancestor sits at depth 0, not depth 3.
    depth: usize,
    /// True when the next heading in document order has a numerically
    /// greater `heading` value (i.e. is nested under this one) — controls
    /// whether this row gets a collapse arrow at all.
    has_children: bool,
}

/// Turns a flat, document-order list of `(line_idx, heading_level, text)`
/// into the actual tree Nav's collapse arrows operate on, using the same
/// "a heading's children are everything up to the next heading at an
/// equal-or-shallower level" rule Markdown/VSCode outline views use for a
/// flat heading list. `collapsed` filters out any heading whose nearest
/// collapsed ancestor's subtree it falls inside — nested collapse state is
/// preserved even while hidden (an inner collapse isn't lost when its
/// outer ancestor re-expands, since `collapsed` itself is untouched here).
fn build_nav_entries(headings: &[(usize, u8, String)], collapsed: &HashSet<usize>) -> Vec<NavEntry> {
    let mut stack: Vec<u8> = Vec::new();
    let mut entries = Vec::new();
    // Once we enter a collapsed heading's subtree, set to the depth its
    // children sit at; cleared the moment we reach something shallower.
    let mut skip_from_depth: Option<usize> = None;

    for (i, (line_idx, level, text)) in headings.iter().enumerate() {
        while stack.last().is_some_and(|top| top >= level) {
            stack.pop();
        }
        let depth = stack.len();

        let is_skipped = skip_from_depth.is_some_and(|skip_depth| depth >= skip_depth);
        if !is_skipped {
            skip_from_depth = None;
            let has_children = headings.get(i + 1).is_some_and(|(_, next_level, _)| next_level > level);
            entries.push(NavEntry { line_idx: *line_idx, text: text.clone(), depth, has_children });
            if has_children && collapsed.contains(line_idx) {
                skip_from_depth = Some(depth + 1);
            }
        }

        stack.push(*level);
    }
    entries
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
        let sidebar_width = state.sidebar_width;
        let _ = state;

        let state_handle = self.state.clone();

        div()
            .flex()
            .flex_col()
            .relative()
            .w(px(sidebar_width))
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
                    // Right-click on empty space (not a file/dir row, which
                    // stop_propagation()s its own right-click first) — "New
                    // File" creates at the tree's root; "Delete" has
                    // nothing to act on.
                    .on_mouse_down(MouseButton::Right, {
                        let state_clone = state_handle.clone();
                        move |ev, _window, cx| {
                            let position = (ev.position.x.as_f32(), ev.position.y.as_f32());
                            state_clone.update(cx, |s, cx| {
                                s.open_file_context_menu(position, FileContextMenuTarget::Background);
                                cx.notify();
                            });
                        }
                    })
                    .children(file_tree.iter().map(|node| {
                        Self::render_node(node, 0, &active_path, p, &state_handle, cx)
                    }))
                    .into_any_element(),
                SidebarMode::Nav => self.render_nav_tree(&state_handle, p, cx),
            })
            .when_some(self.state.read(cx).file_context_menu.clone(), |el, menu| {
                el.child(Self::render_context_menu(menu, p, &state_handle, cx))
            })
            // ── Resize handle ────────────────────────────────────────────────
            // A thin strip on the sidebar's right edge (the border shared
            // with the text editor). Dragging it fires MainWindow's
            // `on_drag_move::<SidebarResizePayload>` (main_window.rs), which
            // covers the whole window so the drag keeps tracking even if
            // the cursor slips off this 4px strip mid-drag.
            .child(
                div()
                    .id("sidebar-resize-handle")
                    .absolute()
                    .top(px(0.0))
                    .right(px(-2.0))
                    .h_full()
                    .w(px(4.0))
                    .cursor_col_resize()
                    .occlude()
                    .on_drag(SidebarResizePayload, |payload: &SidebarResizePayload, _offset, _window, cx| {
                        cx.new(|_| payload.clone())
                    }),
            )
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

#[cfg(test)]
mod tests {
    // Import only what's needed, not `super::*` — file_explorer.rs has
    // `use gpui::*;` at module scope, and gpui exports its own `test`
    // attribute macro (for async GPUI tests) that shadows std's `#[test]`
    // and sends the test-attribute expansion into infinite recursion if
    // it's in scope here (same fix as text_editor.rs's own test module).
    use super::build_nav_entries;
    use std::collections::HashSet;

    /// (line_idx, heading_level, text) shorthand matching build_nav_entries'
    /// own input shape, so test cases read as close to real headings as
    /// possible without pulling in AppState/Paragraph.
    fn h(line_idx: usize, level: u8, text: &str) -> (usize, u8, String) {
        (line_idx, level, text.to_string())
    }

    #[test]
    fn flat_siblings_all_depth_zero_no_children() {
        // Three Pockets in a row: none nests under another.
        let headings = vec![h(0, 1, "A"), h(1, 1, "B"), h(2, 1, "C")];
        let entries = build_nav_entries(&headings, &HashSet::new());

        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|e| e.depth == 0));
        assert!(entries.iter().all(|e| !e.has_children));
    }

    #[test]
    fn strictly_increasing_levels_nest_and_flag_has_children() {
        // Pocket > Hat > Block > Tag, each nested one deeper than the last.
        let headings = vec![h(0, 1, "Pocket"), h(1, 2, "Hat"), h(2, 3, "Block"), h(3, 4, "Tag")];
        let entries = build_nav_entries(&headings, &HashSet::new());

        let depths: Vec<usize> = entries.iter().map(|e| e.depth).collect();
        assert_eq!(depths, vec![0, 1, 2, 3]);
        assert_eq!(
            entries.iter().map(|e| e.has_children).collect::<Vec<_>>(),
            vec![true, true, true, false],
        );
    }

    #[test]
    fn a_tag_with_no_ancestor_sits_at_depth_zero_not_its_type_level() {
        // A lone Tag (heading level 4) with nothing before it has no
        // ancestor at all — it should render flush left, not indented as
        // if it were nested three levels deep by raw type.
        let headings = vec![h(0, 4, "orphan tag")];
        let entries = build_nav_entries(&headings, &HashSet::new());

        assert_eq!(entries[0].depth, 0);
        assert!(!entries[0].has_children);
    }

    #[test]
    fn sibling_after_deeper_subtree_pops_back_to_correct_depth() {
        // Pocket > Hat > Block, then a second Pocket at the top level again —
        // the second Pocket must be depth 0, not still nested under the first.
        let headings = vec![
            h(0, 1, "Pocket A"),
            h(1, 2, "Hat under A"),
            h(2, 3, "Block under Hat"),
            h(3, 1, "Pocket B"),
        ];
        let entries = build_nav_entries(&headings, &HashSet::new());

        assert_eq!(entries[3].line_idx, 3);
        assert_eq!(entries[3].depth, 0);
        assert!(entries[0].has_children); // Pocket A has Hat/Block nested under it
    }

    #[test]
    fn collapsing_a_heading_hides_only_its_own_subtree() {
        let headings = vec![
            h(0, 1, "Pocket A"),
            h(1, 2, "Hat under A"),
            h(2, 3, "Block under Hat"),
            h(3, 1, "Pocket B"),
            h(4, 2, "Hat under B"),
        ];
        let mut collapsed = HashSet::new();
        collapsed.insert(0); // collapse "Pocket A"

        let entries = build_nav_entries(&headings, &collapsed);
        let visible: Vec<usize> = entries.iter().map(|e| e.line_idx).collect();

        // Pocket A itself still shows (with its arrow); its Hat/Block are
        // hidden. Pocket B and its own Hat are untouched.
        assert_eq!(visible, vec![0, 3, 4]);
    }

    #[test]
    fn collapsing_outer_heading_preserves_inner_collapse_state() {
        // A Pocket containing a Hat, which itself contains a Block. Collapse
        // both; only the Pocket's collapse should determine what's hidden
        // right now, but the Hat's own collapsed flag must survive being
        // hidden, so re-expanding the Pocket alone doesn't also silently
        // re-expand the Hat.
        let headings = vec![
            h(0, 1, "Pocket"),
            h(1, 2, "Hat"),
            h(2, 3, "Block"),
        ];
        let mut collapsed = HashSet::new();
        collapsed.insert(0); // Pocket collapsed
        collapsed.insert(1); // Hat (currently hidden) also collapsed

        // While the Pocket is collapsed, only it is visible.
        let entries = build_nav_entries(&headings, &collapsed);
        assert_eq!(entries.iter().map(|e| e.line_idx).collect::<Vec<_>>(), vec![0]);

        // Re-expand just the Pocket (remove line 0 from the set) — the Hat's
        // own collapse (line 1, never removed) should still hide the Block.
        collapsed.remove(&0);
        let entries = build_nav_entries(&headings, &collapsed);
        assert_eq!(entries.iter().map(|e| e.line_idx).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn non_collapsed_heading_with_no_children_shows_no_arrow() {
        let headings = vec![h(0, 1, "Pocket"), h(1, 4, "Tag under it")];
        let entries = build_nav_entries(&headings, &HashSet::new());

        assert!(entries[0].has_children); // Pocket has the Tag nested under it
        assert!(!entries[1].has_children); // Tag itself has nothing nested under it
    }
}
