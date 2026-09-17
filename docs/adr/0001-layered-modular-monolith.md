# ADR 0001: Layered modular monolith

- Status: accepted
- Date: 2026-09-15

## Decision

Keep one GPUI application and one root `AppState`, split into crate-private feature modules.
Views render state and dispatch `AppCommand`; `AppEffect` adapters own filesystem and
background work. Repository traits form the test boundary.

Rich `DocumentBuffer` paragraphs are the only writable document text. `PlainTextIndex`
is derived, cached, and invalidated by `DocumentBuffer` mutation. Undo and recovery
persist paragraph snapshots rather than a second flat-text representation.

All `AppState` fields are private to the state module. Views use read-only selectors
and focused mutation methods; private session scalars need not become new aggregates.
Existing UI-substate mutable accessors remain scoped exceptions, not mutable access
to the entire application model.

The public crate surface remains `app::run` plus the hidden integration-test boundary.

## Consequences

This avoids a rewrite and keeps platform APIs at the edge. `AppState` remains a broad
composition root, so new behavior belongs in an existing feature module rather than in
`main_window.rs`, `editor/view.rs`, or `state/editing.rs` unless it coordinates features.
