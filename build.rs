// closed_beta_plan.md §3: bake the git short-SHA in at build time so a
// crash report or bug report can be tied back to the exact commit that
// produced the build (`env!("VIMBATIM_GIT_SHA")` in src/main.rs and
// src/settings_modal.rs). Falls back to "unknown" when git isn't available
// (e.g. building from a source tarball with no .git directory) rather than
// failing the build over a cosmetic string.
fn main() {
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=VIMBATIM_GIT_SHA={sha}");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
