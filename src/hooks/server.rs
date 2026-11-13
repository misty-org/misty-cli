//! misty-server's checks. A commit runs the suites its staged changes touch; a push first
//! refuses history the public repository must not contain, then runs every suite and the secret
//! scan on a clean tree.

use std::{fs, path::Path};

use anyhow::{bail, Result};

use super::{
    runner,
    secrets::{self, Scan},
};
use crate::process::{npm, CommandSpec};

pub const ALL: &[&str] = &["secrets", "go", "collab"];

const SCAN: Scan = Scan {
    config: Some(".gitleaks.toml"),
    unpushed_without_upstream: true,
};

pub fn suite(root: &Path, name: &str) -> Result<()> {
    match name {
        "go" => {
            crate::checks::go_file_sizes(root)?;
            super::server_tests::run(root, &[])
        }
        "collab" => collab(root),
        "secrets" => secrets::commits(root, &SCAN),
        "staged-secrets" => secrets::staged(root, &SCAN),
        _ => bail!("Unknown suite {name}; misty-server has go, collab, secrets and all"),
    }
}

fn collab(root: &Path) -> Result<()> {
    let worker = root.join("services/journal-collab");
    let modified = |path: &str| fs::metadata(worker.join(path)).and_then(|meta| meta.modified()).ok();
    // Reinstalling takes longer than the tests, so only when the lockfile moved.
    let installed = match (
        modified("node_modules/.package-lock.json"),
        modified("package-lock.json"),
    ) {
        (Some(installed), Some(lockfile)) => installed > lockfile,
        _ => false,
    };
    let mut commands = Vec::new();
    if !installed {
        commands.push(CommandSpec::new(npm()).args(["ci", "--no-audit", "--no-fund", "--silent"]));
    }
    for script in ["typecheck", "test", "test:runtime"] {
        commands.push(CommandSpec::new(npm()).args(["run", script]));
    }
    runner::commands(&worker, commands)
}

pub fn suites_for<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let (mut collab, mut go) = (false, false);
    for path in paths {
        collab |= path.starts_with("services/journal-collab/");
        go |= path.ends_with(".go")
            || matches!(path, "go.mod" | "go.sum")
            || path.starts_with("scripts/");
    }
    [(collab, "collab"), (go, "go")]
        .into_iter()
        .filter_map(|(touched, name)| touched.then_some(name))
        .collect()
}

pub fn pre_commit(root: &Path) -> Result<()> {
    super::require_staged_snapshot(root)?;
    secrets::staged(root, &SCAN)?;
    let staged = super::staged_paths(root)?;
    for name in suites_for(staged.lines()) {
        runner::run(name, || suite(root, name))?;
    }
    Ok(())
}

pub fn pre_push(root: &Path, refs: &str) -> Result<()> {
    super::check_public_history(root, "scripts/check-public-history.py", refs)?;
    super::require_clean_tree(root, true)?;
    for name in ALL {
        runner::run(name, || suite(root, name))?;
    }
    Ok(())
}
