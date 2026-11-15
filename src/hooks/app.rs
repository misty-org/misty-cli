//! misty's checks. A commit runs every suite its staged changes touch, cheapest first; a push
//! runs none (each commit was checked when it was made) and only guards what may be published.
//! `MISTY_COMMIT_SECONDS=<n>` caps a commit's checks for once; whatever is cut off goes unchecked.

use std::{path::Path, time::Instant};

use anyhow::{bail, Result};

use super::{
    runner::{self, Step},
    secrets::{self, Scan},
};
use crate::process::{npm, CommandSpec};

pub const ALL: &[&str] = &["secrets", "scripts", "rust-desktop", "frontend"];
/// Also compiles for Windows; run it when cutting a release.
pub const RELEASE: &[&str] = &["secrets", "scripts", "rust-desktop", "frontend", "desktop-windows"];

const SCAN: Scan = Scan {
    config: Some(".config/gitleaks.toml"),
    unpushed_without_upstream: true,
};

pub fn suite(root: &Path, name: &str) -> Result<()> {
    match name {
        "frontend" => runner::in_turn(
            root,
            vec![
                Step::new("frontend-typecheck", [npm_run("typecheck")]),
                Step::new("frontend-lint", [npm_run("lint")]),
                Step::new(
                    "frontend-unit",
                    [CommandSpec::new(npm()).arg("test").env("MISTY_SHARED_CPU", "1")],
                ),
            ],
        ),
        // Injected page scripts and the dev runner (tasks/).
        "scripts" => runner::commands(root, [npm_run("test:scripts")]),
        // Tests only: compiling for other platforms and building examples wait for a release.
        "rust-desktop" => runner::in_turn(root, rust_desktop()),
        "desktop-windows" => desktop_windows(root),
        "secrets" => secrets::commits(root, &SCAN),
        "staged-secrets" => secrets::staged(root, &SCAN),
        _ => bail!("Unknown suite {name}; misty has frontend, scripts, rust-desktop, desktop-windows, secrets, all and release"),
    }
}

fn npm_run(script: &str) -> CommandSpec {
    CommandSpec::new(npm()).args(["run", script])
}

fn cargo_test(manifest: &str) -> CommandSpec {
    CommandSpec::new("cargo").args([
        "test",
        "--locked",
        "--quiet",
        "--tests",
        "--manifest-path",
        manifest,
    ])
}

fn rust_desktop() -> Vec<Step> {
    vec![
        Step::new("desktop-crate", [cargo_test("src-tauri/Cargo.toml")]),
        // The crates the app builds on: the file gate, the database and documents.
        Step::new(
            "base-crates",
            ["file_access", "database", "documents"]
                .map(|name| cargo_test(&format!("src-tauri/features/{name}/Cargo.toml"))),
        ),
        Step::new(
            "browser-data-crate",
            [cargo_test("src-tauri/features/sync/browser_data/Cargo.toml")],
        ),
        Step::new(
            "agent-crate",
            [
                cargo_test("src-tauri/features/browser/page_text/Cargo.toml"),
                cargo_test("src-tauri/features/agents/runs/Cargo.toml"),
            ],
        ),
        // Kiri's core tests need no Tauri; the desktop crate builds its native side.
        Step::new(
            "kiri-crate",
            [cargo_test("src-tauri/features/kiri/Cargo.toml").arg("--no-default-features")],
        ),
        // The workers the app embeds: each runs in an OS sandbox and can't link Tauri.
        Step::new(
            "desktop-workers",
            [
                "src-tauri/features/documents/worker/Cargo.toml",
                "src-tauri/features/browser/page_text/worker/Cargo.toml",
                "src-tauri/features/agents/shell/worker/Cargo.toml",
            ]
            .map(cargo_test),
        ),
    ]
}

/// The desktop crate compiles for Windows too, and every example with it.
fn desktop_windows(root: &Path) -> Result<()> {
    if cfg!(windows) {
        println!("Covered by desktop-crate on Windows.");
        return Ok(());
    }
    if !CommandSpec::new("cargo")
        .args(["xwin", "--version"])
        .succeeds(root)
    {
        bail!("cargo-xwin is required to check the Windows build: cargo install --locked cargo-xwin\nand rustup target add x86_64-pc-windows-msvc");
    }
    runner::commands(
        root,
        [CommandSpec::new("cargo").args([
            "xwin",
            "check",
            "--locked",
            "--quiet",
            "--all-targets",
            "--target",
            "x86_64-pc-windows-msvc",
            "--manifest-path",
            "src-tauri/Cargo.toml",
        ])],
    )
}

/// The suites a set of changed paths touches, in the order they run.
pub fn suites_for<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let (mut frontend, mut desktop, mut scripts) = (false, false, false);
    for path in paths {
        let configuration = path.starts_with("src/")
            || path.starts_with(".config/")
            || matches!(path, "package.json" | "package-lock.json")
            || (path.starts_with("tsconfig") && path.ends_with(".json") && !path.contains('/'));
        if configuration {
            frontend = true;
            scripts = true;
        } else if path.starts_with("tasks/") {
            scripts = true;
        } else if path.starts_with("src-tauri/") || path == "rust-toolchain.toml" {
            desktop = true;
        }
    }
    [
        (scripts, "scripts"),
        (desktop, "rust-desktop"),
        (frontend, "frontend"),
    ]
    .into_iter()
    .filter_map(|(touched, name)| touched.then_some(name))
    .collect()
}

pub fn pre_commit(root: &Path) -> Result<()> {
    let started = Instant::now();
    super::require_staged_snapshot(root)?;
    runner::run("staged-secrets", || suite(root, "staged-secrets"))?;
    let staged = super::staged_paths(root)?;
    let suites = suites_for(staged.lines());
    if suites.is_empty() {
        println!("No test suites affected.");
        return Ok(());
    }
    let limit: u64 = std::env::var("MISTY_COMMIT_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    // What's left of the limit. Never 0, which would mean no limit at all.
    let remaining = if limit > 0 {
        limit.saturating_sub(started.elapsed().as_secs()).max(1)
    } else {
        0
    };
    runner::sequence(root, remaining, &suites)
}

pub fn pre_push(root: &Path, refs: &str) -> Result<()> {
    super::check_public_history(root, ".githooks/check-public-history.py", refs)
}
