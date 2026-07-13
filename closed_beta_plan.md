# Closed Beta Packaging Plan

## Goal
Get installable Windows and macOS builds of Vimbatim into the hands of a small
set of non-technical debater testers, distributed manually (Drive/Discord/
email — no public GitHub Release), with a way to hear about crashes and bugs
as they happen.

Platforms: **Windows + macOS**. Linux is skipped for the beta.

---

## 0. Prerequisite fix: settings.conf / working_directory resolve against CWD — **Done**

`src/state.rs`: `settings_conf_path()` resolves next to
`std::env::current_exe()`'s parent directory (falls back to the bare
relative path only if `current_exe()` itself fails); `default_working_directory()`
prefers `$HOME` (`%USERPROFILE%\Documents` on Windows) over
`current_dir()`, since nothing persists a prior `working_directory` across
launches for it to prefer instead. `main.rs`'s own `Keybinds::load` call
now goes through the same `settings_conf_path()` rather than its own
separate `Path::new("settings.conf")` literal, so the two settings.conf
readers in the codebase can't drift out of agreement. 2 new tests, 658/658
pass, clean build, `timeout 5 ./run.sh` ran clean with no panic.

**Still needs the real verification this fix exists for**: a double-click
launch from outside the repo directory on a real machine (this sandbox has
no display) — the actual bug scenario `cargo run`/`./run.sh` can't
reproduce, since both are always invoked from the repo root.

**This has to land before the first beta build — packaging around it would just
ship a broken first-launch experience.**

Today:
- `main.rs:42` loads keybinds via `Keybinds::load(Path::new("settings.conf"))`
- `state.rs:610` loads formatting settings the same way, via a bare relative path
- `state.rs:606` defaults `working_directory` to `std::env::current_dir()`

All three resolve against the process's current working directory, which is
always the repo root today because that's where `cargo run`/`./run.sh` is
invoked from. A double-clicked `.app` or `.exe` has no such guarantee — macOS
Finder launches typically start with CWD `/`, and Windows shortcuts depend on
the "Start in" field. A tester's first launch would silently fall back to
default keybinds (their `settings.conf` wouldn't be found) and open the file
tree at some unrelated system directory.

**Fix:**
- Resolve `settings.conf` relative to `std::env::current_exe()`'s parent
  directory (next to the binary), not CWD.
- Default `working_directory` to the user's home directory (or `Documents` on
  Windows) when no prior working directory is known, instead of raw
  `current_dir()`.
- Ship a `settings.conf` alongside the packaged binary (same file that's in
  the repo today) so first launch has real defaults to read.

This is a small, self-contained change to `main.rs`/`state.rs` — do it as its
own commit, verified with the existing test suite, before touching any
packaging tooling.

---

## 1. Build tooling per platform

### macOS — `.app` bundle in a `.dmg`
- Use [`cargo-bundle`](https://github.com/burtonageo/cargo-bundle) to produce
  a real `Vimbatim.app` (icon, `Info.plist`, bundled `settings.conf` as a
  bundled resource).
- Wrap the `.app` in a `.dmg` via `hdiutil create` (standard drag-to-
  Applications experience — a bare `.app` zipped up is a worse first
  impression and Gatekeeper handles `.dmg`s more predictably).
- Needs an `.icns` icon — reuse/derive from whatever app icon exists, or a
  placeholder for the beta if none does yet.

### Windows — plain `.zip`
- `cargo build --release` produces `vimbatim.exe` directly. Windows doesn't
  need a wrapper to be double-clickable, so skip an MSI/WiX installer for the
  beta — it's real added complexity (WiX authoring, upgrade codes) for little
  benefit at closed-beta scale.
- Zip `vimbatim.exe` + `settings.conf` + a one-page `README.txt` ("unzip
  anywhere, double-click vimbatim.exe").
- Revisit a real installer only if testers report the zip-and-run flow is a
  problem.

---

## 2. Build infrastructure: GitHub Actions matrix

GPUI's platform backends (Metal on macOS, DirectX/Vulkan on Windows) are
native bindings — cross-compiling either from Linux isn't realistic. Build
natively on each OS via GitHub Actions:

```yaml
# .github/workflows/beta-build.yml (sketch)
on: workflow_dispatch          # manually triggered per beta build, not on every push
jobs:
  build:
    strategy:
      matrix:
        os: [windows-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo build --release
      # + cargo-bundle / dmg step on macos, zip step on windows
      - uses: actions/upload-artifact@v4
        with:
          name: vimbatim-${{ matrix.os }}
          path: <dmg-or-zip>
```

- `workflow_dispatch`-triggered (not on every push) — beta builds are cut
  deliberately, not on every commit, so testers aren't getting noise.
- Artifacts from a workflow run are only visible to repo collaborators —
  this satisfies "not a public Release" while still getting free native
  build machines for both OSes.
- You download the two artifacts from the completed Actions run and hand
  them out manually (Drive link, Discord, email) — no separate hosting to
  stand up or maintain.

---

## 3. Versioning / build labeling

Every beta build needs to be traceable back to an exact commit, since crash
reports and bug reports are useless without knowing which build produced
them.

- Bump `Cargo.toml`'s `version` per beta round (e.g. `0.1.0-beta.1`,
  `0.1.0-beta.2`, ...).
- Bake the git short-SHA in at build time via `build.rs` reading
  `git rev-parse --short HEAD` into an env var, exposed in-app as
  `env!("VIMBATIM_GIT_SHA")`.
- Surface `{version} ({git_sha})` somewhere always-visible but out of the
  way — the Settings modal is the natural spot (it already exists per
  `settings_modal.rs`) — so a tester reporting a bug can screenshot or read
  off exactly what build they're on.

---

## 4. No code signing for the beta

Both OSes will show an "unknown developer" warning on first launch —
Gatekeeper on macOS, SmartScreen on Windows. A paid Apple Developer account
($99/yr, needed for notarization) and a Windows code-signing cert are both
real costs not worth taking on before the beta validates there's demand.

Ship a short **"First Launch" instructions** doc with the build, covering:

- **macOS:** right-click the app → "Open" → "Open" again in the warning
  dialog (bypasses Gatekeeper without disabling it system-wide). If macOS
  still refuses, `xattr -cr /Applications/Vimbatim.app` from Terminal clears
  the quarantine flag.
- **Windows:** SmartScreen dialog → "More info" → "Run anyway."

Revisit signing if the beta grows past a small trusted group, since the
warning dialogs are real friction for non-technical users even with
instructions.

---

## 5. Crash logging + feedback loop

A double-clicked GUI app has no visible console — today, an unhandled panic
is completely silent to the tester (the app just vanishes), which makes bug
reports impossible to act on.

- Install a `std::panic::set_hook` in `main.rs` that, in addition to the
  default behavior, writes the panic message + backtrace + the build's
  version/git-sha string to a fixed log file (e.g.
  `~/.vimbatim/crash.log` on macOS, `%APPDATA%\vimbatim\crash.log` on
  Windows — both writable without extra permissions).
- Feedback process (no new infra needed): a single shared Discord channel or
  a Google Form for testers to describe what happened and attach
  `crash.log` if the app died. Pin the exact file path for each OS so
  testers don't have to hunt for it.
- Optional stretch, not required for beta launch: a "Copy Crash Log Path"
  button in the Settings modal, since `settings_modal.rs` already has a
  natural place to add it.

---

## 6. Rollout

1. Land the CWD-resolution fix (§0), verify with `cargo test` + a real
   double-click launch from outside the repo directory (the actual bug
   scenario, not `cargo run`).
2. Wire up `cargo-bundle`/dmg (macOS) and the zip step (Windows) locally
   first, confirm both launch clean on a real machine.
3. Add the GitHub Actions workflow (§2), do one manual `workflow_dispatch`
   run, download and sanity-check both artifacts.
4. Add the panic hook + version string (§3, §5).
5. Write the tester-facing "First Launch" doc (§4) and pick the feedback
   channel (§5).
6. Cut beta.1, hand it to a first small batch (2-3 testers) before wider
   distribution — cheaper to catch a packaging mistake with 3 people than
   with the full closed-beta list.
