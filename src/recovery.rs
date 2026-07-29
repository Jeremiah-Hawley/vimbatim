use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Per-OS base directory for everything Vimbatim writes outside the user's
/// documents: `~/.vimbatim` on macOS/Linux, `%APPDATA%\vimbatim` on Windows
/// (`closed_beta_plan.md` §5). Both are writable without extra permissions,
/// unlike the install directory a packaged `.app`/`.exe` may live in.
///
/// Extracted from `state::crash_log_path`, which now calls this rather than
/// duplicating the per-OS branch.
pub fn app_data_dir() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from).map(|dir| dir.join("vimbatim"))
    } else {
        std::env::var_os("HOME").map(PathBuf::from).map(|home| home.join(".vimbatim"))
    };
    base.unwrap_or_else(std::env::temp_dir)
}

/// Where crash snapshots of unsaved tabs live, one `.docx`/`.meta` pair per
/// dirty tab.
pub fn recovery_dir() -> PathBuf {
    app_data_dir().join("recovery")
}

/// Unix seconds at which this process started snapshotting, sampled once.
///
/// The pid alone does not identify a run: pids recycle (`pid_max` is 32768 on
/// many Linux installs, and containers hand out low numbers immediately), so
/// a relaunch that lands on a crashed instance's pid would write tab 0's
/// snapshot straight over that instance's pending recovery entry — and tab 0
/// exists in every session, which makes that the likely collision rather than
/// an exotic one. Pairing the pid with the launch time makes the stem unique
/// per run, and lets `scan_recovery_dir` tell "my own live snapshot" apart
/// from "a dead predecessor that happened to share my pid".
fn launch_secs() -> u64 {
    static LAUNCH_SECS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *LAUNCH_SECS.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

/// Filename stem for one tab's snapshot pair: `<pid>-<launch_secs>-<tab_id>`.
///
/// Takes its parts as arguments rather than reading the process globals so
/// the format and its inverse, `parse_stem`, can be tested against runs other
/// than the current one.
fn stem_for(pid: u32, launch_secs: u64, tab_id: usize) -> String {
    format!("{pid}-{launch_secs}-{tab_id}")
}

/// Reads the pid and launch time back out of a filename stem produced by
/// `stem_for`. `None` for anything else in the directory.
///
/// Deliberately ignores everything after the second segment, so it also
/// parses the stem of an in-flight temp file (`<stem>.docx.<pid>.<n>.tmp`,
/// whose `file_stem()` is `<stem>.docx.<pid>.<n>`).
fn parse_stem(stem: &str) -> Option<(u32, u64)> {
    let mut parts = stem.split('-');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Whether the run that wrote `stem` is still running, i.e. whether its
/// snapshot is a live working file rather than the leftovers of a crash.
///
/// Linux only: it tests for `/proc/<pid>`. Every other platform reports
/// `false`, meaning every snapshot on disk is treated as abandoned — the
/// behaviour before this check existed. A real cross-platform liveness test
/// needs a syscall crate, which this project does not depend on; on macOS and
/// Windows a second running instance can therefore still consume the first
/// one's snapshots, as it always could.
fn stem_is_live(stem: &str) -> bool {
    let Some((pid, launch)) = parse_stem(stem) else { return false };
    if pid == std::process::id() {
        // Our own pid proves nothing on its own — we may have inherited it
        // from the very instance whose snapshot this is. Only a matching
        // launch stamp makes it ours.
        return launch == launch_secs();
    }
    cfg!(target_os = "linux") && Path::new("/proc").join(pid.to_string()).exists()
}

/// The `.docx` and `.meta` pair for one tab, as
/// `<recovery_dir>/<pid>-<launch_secs>-<tab_id>.{docx,meta}`.
pub fn snapshot_paths(tab_id: usize) -> (PathBuf, PathBuf) {
    let dir = recovery_dir();
    let stem = stem_for(std::process::id(), launch_secs(), tab_id);
    (dir.join(format!("{stem}.docx")), dir.join(format!("{stem}.meta")))
}

/// One recoverable document found in `recovery_dir()` at launch.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryEntry {
    pub snapshot: PathBuf,
    pub meta: PathBuf,
    /// The file the tab was editing, or `None` for a tab that had never been
    /// saved. `None` makes Resume open an untitled modified tab and makes
    /// Save As the only way to give it a home.
    pub original_path: Option<PathBuf>,
    pub title: String,
    /// Unix seconds. Used only to sort entries newest-first — deliberately
    /// not rendered as a date, since that would need a date-formatting
    /// dependency and the title plus original path already identify the
    /// document.
    pub saved_at: u64,
}

/// Serialises snapshot metadata as flat `key=value` lines, the same shape
/// settings.conf uses. `original_path` is omitted entirely (rather than
/// written empty) for a never-saved tab, so `parse_meta` can distinguish
/// "no path" from "empty path".
pub fn format_meta(original_path: Option<&Path>, title: &str, saved_at: u64) -> String {
    let mut out = String::new();
    if let Some(path) = original_path {
        out.push_str(&format!("original_path={}\n", path.display()));
    }
    out.push_str(&format!("title={title}\n"));
    out.push_str(&format!("saved_at={saved_at}\n"));
    out
}

/// Tolerant `key=value` scan mirroring `state::load_working_directory`:
/// unknown keys and blank lines are ignored so a future key can be added
/// without invalidating existing snapshots. Returns `None` when a required
/// key is missing or `saved_at` doesn't parse — the caller treats that as a
/// corrupt entry and deletes it.
pub fn parse_meta(contents: &str) -> Option<(Option<PathBuf>, String, u64)> {
    let mut original_path = None;
    let mut title = None;
    let mut saved_at = None;
    for line in contents.lines() {
        // split_once splits on the FIRST '=', so a path containing '=' survives.
        let Some((key, value)) = line.split_once('=') else { continue };
        match key.trim() {
            "original_path" => original_path = Some(PathBuf::from(value.trim())),
            "title" => title = Some(value.trim().to_string()),
            "saved_at" => saved_at = value.trim().parse::<u64>().ok(),
            _ => {}
        }
    }
    Some((original_path, title?, saved_at?))
}

/// A snapshot may consume at most `1/SNAPSHOT_TIME_BUDGET` of wall time, so
/// an expensive document snapshots proportionally less often than a cheap
/// one. 20 means 5%.
const SNAPSHOT_TIME_BUDGET: u32 = 20;
/// Never snapshot more often than this, however cheap the write is — below
/// a few seconds the writes stop buying meaningfully less data loss.
pub const MIN_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(3);
/// Never snapshot less often than this, however expensive the write is.
/// Caps worst-case loss (to `kill -9` or power loss only — the panic hook
/// ignores this interval entirely and always snapshots).
pub const MAX_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60);

/// How long a tab must sit idle before its next snapshot, derived from what
/// its previous snapshot actually cost.
///
/// Self-tuning: any change that makes writes cheaper (e.g. switching the
/// snapshot to `CompressionMethod::Stored`) feeds straight back in here and
/// pulls the interval down toward the floor with no constant to retune.
pub fn snapshot_interval(last_cost: Option<Duration>) -> Duration {
    match last_cost {
        // Nothing measured yet: assume cheap. One write corrects it.
        None => MIN_SNAPSHOT_INTERVAL,
        Some(cost) => (cost * SNAPSHOT_TIME_BUDGET)
            .clamp(MIN_SNAPSHOT_INTERVAL, MAX_SNAPSHOT_INTERVAL),
    }
}

/// Whether this tab is due for a snapshot: it has unsaved changes, those
/// changes are newer than what is already on disk, and the user has stopped
/// typing for at least `interval`.
///
/// Takes loose fields rather than `&Tab` so this module needs no `state`
/// import and the predicate can be tested without building a whole `Tab`.
pub fn needs_snapshot(
    is_modified: bool,
    content_version: u64,
    last_snapshot_version: u64,
    last_edit_at: Option<Instant>,
    now: Instant,
    interval: Duration,
) -> bool {
    if !is_modified || content_version == last_snapshot_version {
        return false;
    }
    /*
     * `None` means "no open idle window", not "no work to save". The
     * `is_modified` guard above has already excluded every never-edited tab,
     * so the only tabs that reach here with `None` are the ones `undo()` and
     * `redo()` produce: they clear `last_edit_at` on purpose to break the
     * undo-coalescing window, while leaving the tab modified with a bumped
     * `content_version`. Defaulting those to "not due" disabled snapshotting
     * for that tab permanently — one Ctrl+Z and nothing was ever written
     * again, so a later crash restored the pre-undo content.
     */
    last_edit_at.map(|t| now.duration_since(t) >= interval).unwrap_or(true)
}

/// Enumerates every recoverable document in `dir`, newest first.
///
/// Snapshots belonging to a process that is still running are skipped
/// entirely — neither surfaced nor deleted (see `stem_is_live`). A second
/// instance launched while the first is editing would otherwise list the
/// first instance's live snapshot, and since the recovery prompt has no
/// dismiss, every one of its three buttons ends in `delete_entry` — wiping
/// the working snapshot of a session that is still going. The first instance
/// never notices: it believes that version is safely on disk and will not
/// rewrite it until the user edits that tab again. Skipping also closes the
/// window where a scan lands between another instance's `.docx` and `.meta`
/// writes and deletes the half-written pair out from under it.
///
/// Self-healing otherwise: a `.docx` with no `.meta` (or vice versa), a pair
/// whose `.meta` fails to parse, and a `.tmp` file abandoned by a write that
/// was killed mid-flight are all deleted rather than surfaced — an entry the
/// prompt cannot describe or restore is worse than no entry. Anything that
/// isn't a `.docx`/`.meta`/`.tmp` file is left alone.
///
/// Takes `dir` as a parameter so tests can point it at a temp directory;
/// production callers pass `&recovery_dir()`.
pub fn scan_recovery_dir(dir: &Path) -> Vec<RecoveryEntry> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new(); // no recovery dir yet — nothing to recover
    };

    let mut stems: Vec<String> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let extension = path.extension().and_then(|e| e.to_str());
        if !matches!(extension, Some("docx") | Some("meta") | Some("tmp")) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if stem_is_live(stem) {
            continue; // another instance is using this — hands off
        }
        if extension == Some("tmp") {
            // Leftover from a write that never reached its rename. Its owner
            // is gone, so nothing will ever finish or clean it up, and the
            // pair-scan below cannot see it — without this it accumulates
            // forever.
            let _ = std::fs::remove_file(&path);
            continue;
        }
        if !stems.iter().any(|s| s == stem) {
            stems.push(stem.to_string());
        }
    }

    let mut entries = Vec::new();
    for stem in stems {
        let snapshot = dir.join(format!("{stem}.docx"));
        let meta = dir.join(format!("{stem}.meta"));

        let parsed = std::fs::read_to_string(&meta).ok().and_then(|c| parse_meta(&c));
        match parsed {
            Some((original_path, title, saved_at)) if snapshot.exists() => {
                entries.push(RecoveryEntry { snapshot, meta, original_path, title, saved_at });
            }
            // Orphan or corrupt: clean it up so it never reappears.
            _ => {
                let _ = std::fs::remove_file(&snapshot);
                let _ = std::fs::remove_file(&meta);
            }
        }
    }

    entries.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
    entries
}

/// Removes both files of a snapshot pair. Missing files are not an error —
/// this is called on every clean-exit path, most of which have no snapshot
/// to remove.
pub fn delete_snapshot(tab_id: usize) {
    let (docx, meta) = snapshot_paths(tab_id);
    let _ = std::fs::remove_file(docx);
    let _ = std::fs::remove_file(meta);
}

/// `delete_snapshot` for an entry read back off disk, whose filename stem
/// belongs to a previous process and so cannot be rebuilt from a tab id.
pub fn delete_entry(entry: &RecoveryEntry) {
    let _ = std::fs::remove_file(&entry.snapshot);
    let _ = std::fs::remove_file(&entry.meta);
}

/// Writes one tab's snapshot pair and returns how long the whole operation
/// took. The caller feeds that duration back into `snapshot_interval`.
///
/// The measurement deliberately spans the entire write, not just the zip
/// call, so any future cost added here is automatically budgeted for.
///
/// Errors are the caller's to swallow: snapshotting is best-effort and must
/// never interrupt editing.
pub fn write_snapshot(
    tab_id: usize,
    paragraphs: &[crate::docx_parser::Paragraph],
    origin: Option<&crate::docx_parser::DocxOrigin>,
    original_path: Option<&Path>,
    title: &str,
) -> std::io::Result<Duration> {
    let started = Instant::now();
    let (docx, meta) = snapshot_paths(tab_id);
    std::fs::create_dir_all(recovery_dir())?;

    let written = match origin {
        Some(origin) => origin.save_snapshot(paragraphs, &docx),
        // No origin: the tab was never a real docx, so build a minimal one
        // from scratch — the same branch `AppState::save_tab` takes.
        None => crate::docx_parser::create_new_docx(paragraphs, &docx),
    };
    if let Err(e) = written {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()));
    }

    let saved_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Meta is written second: a crash between the two leaves an orphaned
    // .docx, which `scan_recovery_dir` cleans up. The reverse order would
    // leave a .meta promising content that was never written.
    std::fs::write(&meta, format_meta(original_path, title, saved_at))?;

    Ok(started.elapsed())
}

/// Flattened copy of the dirty tabs, refreshed by the background snapshot
/// task, read by the panic hook on the way down.
///
/// A `Mutex` rather than a channel because the hook runs on the panicking
/// thread and must finish its writes before the process dies — there is no
/// receiver left to drain a queue.
///
/// ponytail: a global. The alternative is threading a handle through GPUI's
/// startup path into a hook installed before GPUI exists, which is more
/// plumbing than this is worth.
pub static PANIC_SNAPSHOT: std::sync::OnceLock<
    std::sync::Arc<std::sync::Mutex<Vec<crate::state::TabSnapshot>>>,
> = std::sync::OnceLock::new();

/// Snapshots every tab in `tabs` immediately, ignoring the adaptive
/// interval entirely.
///
/// The debounce exists to bound the cost of writes that happen
/// continuously, which does not apply to a write that happens once as the
/// process dies. Every failure is swallowed: a panicking process has no way
/// to report one, and a partial recovery beats none.
pub fn write_all_snapshots(tabs: &[crate::state::TabSnapshot]) {
    for tab in tabs {
        let _ = write_snapshot(
            tab.id,
            &tab.paragraphs,
            tab.origin.as_deref(),
            tab.file_path.as_deref(),
            &tab.title,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pid no live process can hold: above `pid_max`'s 4194304 ceiling on
    /// 64-bit Linux. Test stems use it so the liveness check in
    /// `scan_recovery_dir` always classes them as abandoned — a small literal
    /// pid like `111` really can be running inside a container.
    const DEAD_PID: u32 = u32::MAX;

    #[test]
    fn recovery_dir_sits_under_app_data_dir() {
        assert_eq!(recovery_dir().parent().unwrap(), app_data_dir());
        assert_eq!(recovery_dir().file_name().unwrap(), "recovery");
    }

    #[test]
    fn snapshot_paths_pair_shares_a_stem_and_differs_only_by_extension() {
        let (docx, meta) = snapshot_paths(7);
        assert_eq!(docx.file_stem(), meta.file_stem());
        assert_eq!(docx.extension().unwrap(), "docx");
        assert_eq!(meta.extension().unwrap(), "meta");
        assert_eq!(docx.parent().unwrap(), recovery_dir());
    }

    #[test]
    fn snapshot_paths_include_the_pid_and_launch_time_so_two_runs_do_not_collide() {
        let (docx, _) = snapshot_paths(7);
        let stem = docx.file_stem().unwrap().to_str().unwrap();
        assert_eq!(stem, stem_for(std::process::id(), launch_secs(), 7));
        assert_eq!(parse_stem(stem), Some((std::process::id(), launch_secs())));
        assert!(stem.ends_with("-7"));
    }

    #[test]
    fn two_launches_of_one_pid_and_tab_get_different_stems() {
        // The whole point of the launch stamp: a relaunch handed a dead
        // instance's pid must not write to that instance's snapshot path.
        assert_ne!(stem_for(4211, 1_750_000_000, 0), stem_for(4211, 1_750_086_400, 0));
    }

    #[test]
    fn scan_returns_an_entry_whose_process_is_dead() {
        // Counterpart to the live-owner test: the liveness check must not be
        // so broad that real crash leftovers stop being offered.
        let dir = temp_dir("dead-owner");
        write_pair(&dir, &stem_for(DEAD_PID, 1_750_000_000, 0), None, "crashed", 1);

        let entries = scan_recovery_dir(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "crashed");
    }

    #[test]
    fn scan_returns_an_entry_left_by_a_dead_predecessor_that_shared_our_pid() {
        // Same pid as us, different launch: the pid was recycled. Treating it
        // as live would hide the crashed document forever.
        let dir = temp_dir("recycled-pid");
        write_pair(&dir, &stem_for(std::process::id(), 1, 0), None, "predecessor", 1);

        let entries = scan_recovery_dir(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "predecessor");
    }

    #[test]
    fn different_tab_ids_get_different_snapshot_paths() {
        assert_ne!(snapshot_paths(1).0, snapshot_paths(2).0);
    }

    #[test]
    fn meta_round_trips_a_tab_with_a_file_path() {
        let text = format_meta(Some(Path::new("/home/j/case.docx")), "case.docx", 1750000000);
        let (path, title, saved_at) = parse_meta(&text).unwrap();
        assert_eq!(path, Some(PathBuf::from("/home/j/case.docx")));
        assert_eq!(title, "case.docx");
        assert_eq!(saved_at, 1750000000);
    }

    #[test]
    fn meta_round_trips_a_never_saved_tab_with_no_path() {
        let text = format_meta(None, "New Tab", 42);
        let (path, title, saved_at) = parse_meta(&text).unwrap();
        assert_eq!(path, None);
        assert_eq!(title, "New Tab");
        assert_eq!(saved_at, 42);
    }

    #[test]
    fn meta_preserves_a_path_containing_an_equals_sign() {
        // split_once('=') must split on the FIRST '=' only, or this path is truncated.
        let weird = Path::new("/home/j/a=b/case.docx");
        let text = format_meta(Some(weird), "case.docx", 1);
        let (path, _, _) = parse_meta(&text).unwrap();
        assert_eq!(path, Some(weird.to_path_buf()));
    }

    #[test]
    fn parse_meta_returns_none_when_title_is_missing() {
        assert!(parse_meta("saved_at=5\n").is_none());
    }

    #[test]
    fn parse_meta_returns_none_when_saved_at_is_missing_or_unparseable() {
        assert!(parse_meta("title=x\n").is_none());
        assert!(parse_meta("title=x\nsaved_at=not-a-number\n").is_none());
    }

    #[test]
    fn parse_meta_ignores_blank_and_unknown_lines() {
        let text = "\ntitle=x\nfuture_key=whatever\n\nsaved_at=9\n";
        let (path, title, saved_at) = parse_meta(text).unwrap();
        assert_eq!(path, None);
        assert_eq!(title, "x");
        assert_eq!(saved_at, 9);
    }

    #[test]
    fn snapshot_interval_uses_the_floor_when_nothing_has_been_measured_yet() {
        assert_eq!(snapshot_interval(None), MIN_SNAPSHOT_INTERVAL);
    }

    #[test]
    fn snapshot_interval_clamps_a_cheap_write_up_to_the_floor() {
        // 10ms * 20 = 200ms, well below the 3s floor.
        assert_eq!(snapshot_interval(Some(Duration::from_millis(10))), MIN_SNAPSHOT_INTERVAL);
    }

    #[test]
    fn snapshot_interval_scales_linearly_between_the_floor_and_the_cap() {
        // 400ms * 20 = 8s, inside [3s, 60s].
        assert_eq!(snapshot_interval(Some(Duration::from_millis(400))), Duration::from_secs(8));
    }

    #[test]
    fn snapshot_interval_clamps_an_expensive_write_down_to_the_cap() {
        // 3s * 20 = 60s exactly; 10s * 20 = 200s, both cap at 60s.
        assert_eq!(snapshot_interval(Some(Duration::from_secs(3))), MAX_SNAPSHOT_INTERVAL);
        assert_eq!(snapshot_interval(Some(Duration::from_secs(10))), MAX_SNAPSHOT_INTERVAL);
    }

    #[test]
    fn needs_snapshot_is_false_for_a_clean_tab() {
        let now = Instant::now();
        let idle = now - Duration::from_secs(30);
        assert!(!needs_snapshot(false, 5, 0, Some(idle), now, MIN_SNAPSHOT_INTERVAL));
    }

    #[test]
    fn needs_snapshot_is_false_while_the_user_is_still_typing() {
        let now = Instant::now();
        let just_typed = now - Duration::from_millis(200);
        assert!(!needs_snapshot(true, 5, 0, Some(just_typed), now, MIN_SNAPSHOT_INTERVAL));
    }

    #[test]
    fn needs_snapshot_is_true_for_a_dirty_tab_that_has_gone_idle() {
        let now = Instant::now();
        let idle = now - Duration::from_secs(5);
        assert!(needs_snapshot(true, 5, 0, Some(idle), now, MIN_SNAPSHOT_INTERVAL));
    }

    #[test]
    fn needs_snapshot_is_false_when_this_version_was_already_written() {
        let now = Instant::now();
        let idle = now - Duration::from_secs(30);
        assert!(!needs_snapshot(true, 5, 5, Some(idle), now, MIN_SNAPSHOT_INTERVAL));
    }

    #[test]
    fn needs_snapshot_is_false_when_no_edit_has_ever_happened() {
        // A freshly constructed tab has last_edit_at == None, but it is also
        // clean — `is_modified` is what excludes it, not the missing edit
        // stamp. Nothing here is worth writing.
        let now = Instant::now();
        assert!(!needs_snapshot(false, 0, 0, None, now, MIN_SNAPSHOT_INTERVAL));
    }

    #[test]
    fn needs_snapshot_is_true_for_a_tab_whose_last_edit_was_an_undo() {
        // undo()/redo() clear last_edit_at to break the coalescing window
        // while leaving the tab modified with a bumped version. Such a tab
        // has unwritten work and must still be due.
        let now = Instant::now();
        assert!(needs_snapshot(true, 2, 0, None, now, MIN_SNAPSHOT_INTERVAL));
    }

    #[test]
    fn a_recovery_entry_from_a_dead_predecessor_with_our_pid_is_not_overwritten() {
        // A relaunch can be handed the pid of a crashed instance. If the stem
        // were the pid alone, this session's tab 0 would write straight over
        // that instance's pending recovery snapshot.
        let predecessor = format!("{}-0.docx", std::process::id());
        assert_ne!(
            snapshot_paths(0).0.file_name().unwrap().to_str().unwrap(),
            predecessor,
        );
    }

    #[test]
    fn scan_skips_and_keeps_an_entry_owned_by_a_live_process() {
        let dir = temp_dir("live-owner");
        let stem = snapshot_paths(0).0.file_stem().unwrap().to_str().unwrap().to_string();
        write_pair(&dir, &stem, None, "live", 1);

        assert!(scan_recovery_dir(&dir).is_empty(), "a live instance's snapshot must not be offered");
        assert!(dir.join(format!("{stem}.docx")).exists(), "and must not be deleted");
        assert!(dir.join(format!("{stem}.meta")).exists());
    }

    #[test]
    fn scan_sweeps_a_dead_processes_leftover_tmp_file() {
        let dir = temp_dir("stale-tmp");
        let stale = dir.join(format!("{DEAD_PID}-0.docx.tmp"));
        std::fs::write(&stale, b"half a zip").unwrap();

        assert!(scan_recovery_dir(&dir).is_empty());
        assert!(!stale.exists(), "a dead process's .tmp leftover should be swept");
    }

    #[test]
    fn scan_keeps_a_live_processes_tmp_file() {
        let dir = temp_dir("live-tmp");
        let stem = snapshot_paths(0).0.file_stem().unwrap().to_str().unwrap().to_string();
        let in_flight = dir.join(format!("{stem}.docx.tmp"));
        std::fs::write(&in_flight, b"half a zip").unwrap();

        let _ = scan_recovery_dir(&dir);
        assert!(in_flight.exists(), "must not delete a running instance's in-progress write");
    }

    #[test]
    fn needs_snapshot_respects_a_longer_interval_for_an_expensive_document() {
        let now = Instant::now();
        let idle = now - Duration::from_secs(10);
        // Idle 10s: due under the 3s floor, not yet due under a 30s interval.
        assert!(needs_snapshot(true, 5, 0, Some(idle), now, MIN_SNAPSHOT_INTERVAL));
        assert!(!needs_snapshot(true, 5, 0, Some(idle), now, Duration::from_secs(30)));
    }

    /// Not a pass/fail test — prints measured snapshot cost so the recovery
    /// spec's "measure before optimising" step has a real number. Run with:
    ///   cargo test --bin vimbatim recovery::tests::bench_diagnostic_snapshot_cost -- --nocapture
    #[test]
    fn bench_diagnostic_snapshot_cost() {
        use crate::docx_parser::{create_new_docx, parse_docx, Paragraph, Run};

        for para_count in [100usize, 1_000, 5_000] {
            let dir = temp_dir(&format!("bench-{para_count}"));
            let source = dir.join("source.docx");

            let paragraphs: Vec<Paragraph> = (0..para_count)
                .map(|i| {
                    let mut p = Paragraph::default();
                    p.runs.push(Run {
                        text: format!("Paragraph {i}: the quick brown fox jumps over the lazy dog. "),
                        ..Run::default()
                    });
                    p
                })
                .collect();
            create_new_docx(&paragraphs, &source).unwrap();
            let (paragraphs, origin) = parse_docx(&source).unwrap();

            // Warm once so the timing is not dominated by first-touch page faults.
            let _ = write_snapshot(9999, &paragraphs, Some(&origin), Some(&source), "bench.docx");
            let cost = write_snapshot(9999, &paragraphs, Some(&origin), Some(&source), "bench.docx").unwrap();

            let bytes = std::fs::metadata(snapshot_paths(9999).0).map(|m| m.len()).unwrap_or(0);
            println!(
                "{para_count:>5} paragraphs: snapshot {:>7.1}ms, {:>8} bytes, interval {:?}",
                cost.as_secs_f64() * 1000.0,
                bytes,
                snapshot_interval(Some(cost)),
            );

            delete_snapshot(9999);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Makes a unique temp dir for one test. Avoids a `tempfile` dependency —
    /// the process id plus the test-supplied tag is enough to keep concurrent
    /// tests from colliding.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vimbatim-rec-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a well-formed `.docx` + `.meta` pair into `dir`. The `.docx`
    /// content is not a real zip — `scan_recovery_dir` never opens it, it only
    /// checks the pair exists.
    fn write_pair(dir: &Path, stem: &str, original: Option<&str>, title: &str, saved_at: u64) {
        std::fs::write(dir.join(format!("{stem}.docx")), b"not-a-real-zip").unwrap();
        std::fs::write(
            dir.join(format!("{stem}.meta")),
            format_meta(original.map(Path::new), title, saved_at),
        ).unwrap();
    }

    #[test]
    fn scan_returns_a_well_formed_pair() {
        let dir = temp_dir("well-formed");
        write_pair(&dir, &format!("{DEAD_PID}-0"), Some("/home/j/case.docx"), "case.docx", 100);

        let entries = scan_recovery_dir(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "case.docx");
        assert_eq!(entries[0].original_path, Some(PathBuf::from("/home/j/case.docx")));
        assert_eq!(entries[0].saved_at, 100);
        assert_eq!(entries[0].snapshot, dir.join(format!("{DEAD_PID}-0.docx")));
        assert_eq!(entries[0].meta, dir.join(format!("{DEAD_PID}-0.meta")));
    }

    #[test]
    fn scan_sorts_newest_first() {
        let dir = temp_dir("sorted");
        write_pair(&dir, &format!("{DEAD_PID}-0"), None, "older", 100);
        write_pair(&dir, &format!("{DEAD_PID}-1"), None, "newer", 200);

        let titles: Vec<_> = scan_recovery_dir(&dir).into_iter().map(|e| e.title).collect();
        assert_eq!(titles, vec!["newer", "older"]);
    }

    #[test]
    fn scan_deletes_and_skips_a_docx_with_no_meta() {
        let dir = temp_dir("orphan-docx");
        std::fs::write(dir.join(format!("{DEAD_PID}-0.docx")), b"x").unwrap();

        assert!(scan_recovery_dir(&dir).is_empty());
        assert!(!dir.join(format!("{DEAD_PID}-0.docx")).exists(), "orphaned .docx should be cleaned up");
    }

    #[test]
    fn scan_deletes_and_skips_a_meta_with_no_docx() {
        let dir = temp_dir("orphan-meta");
        std::fs::write(dir.join(format!("{DEAD_PID}-0.meta")), format_meta(None, "x", 1)).unwrap();

        assert!(scan_recovery_dir(&dir).is_empty());
        assert!(!dir.join(format!("{DEAD_PID}-0.meta")).exists(), "orphaned .meta should be cleaned up");
    }

    #[test]
    fn scan_deletes_and_skips_a_pair_whose_meta_is_corrupt() {
        let dir = temp_dir("corrupt-meta");
        std::fs::write(dir.join(format!("{DEAD_PID}-0.docx")), b"x").unwrap();
        std::fs::write(dir.join(format!("{DEAD_PID}-0.meta")), "this is not key=value data at all").unwrap();

        assert!(scan_recovery_dir(&dir).is_empty());
        assert!(!dir.join(format!("{DEAD_PID}-0.docx")).exists());
        assert!(!dir.join(format!("{DEAD_PID}-0.meta")).exists());
    }

    #[test]
    fn scan_ignores_unrelated_files() {
        let dir = temp_dir("unrelated");
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        write_pair(&dir, &format!("{DEAD_PID}-0"), None, "real", 1);

        let entries = scan_recovery_dir(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "real");
        assert!(dir.join("notes.txt").exists(), "unrelated files must not be deleted");
    }

    #[test]
    fn scan_of_a_missing_directory_is_empty_not_a_panic() {
        let dir = std::env::temp_dir().join("vimbatim-rec-test-does-not-exist");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(scan_recovery_dir(&dir).is_empty());
    }

    #[test]
    fn delete_entry_removes_both_files_and_tolerates_a_missing_one() {
        let dir = temp_dir("delete");
        write_pair(&dir, &format!("{DEAD_PID}-0"), None, "x", 1);
        let entry = scan_recovery_dir(&dir).pop().unwrap();

        delete_entry(&entry);
        assert!(!entry.snapshot.exists());
        assert!(!entry.meta.exists());

        delete_entry(&entry); // second call must not panic
    }

    #[test]
    fn write_all_snapshots_writes_a_pair_per_tab() {
        use crate::docx_parser::{Paragraph, Run};

        let mut para = Paragraph::default();
        para.runs.push(Run { text: "panic content".into(), ..Default::default() });
        let mirror = vec![crate::state::TabSnapshot {
            id: 4242,
            paragraphs: vec![para],
            origin: None,
            file_path: None,
            title: "New Tab".into(),
        }];
        delete_snapshot(4242);

        write_all_snapshots(&mirror);

        let (docx, meta) = snapshot_paths(4242);
        assert!(docx.exists());
        assert!(meta.exists());
        let (path, title, _) = parse_meta(&std::fs::read_to_string(&meta).unwrap()).unwrap();
        assert_eq!(path, None);
        assert_eq!(title, "New Tab");

        delete_snapshot(4242);
    }
}
