//! misty-sdk's checks: its own `npm run check` (the license rule, lint, build and tests) when a
//! commit touches the packages, and on a push, followed by the docs build, since the docs site reads
//! this repository's `docs/`.

use std::path::Path;

use anyhow::{bail, Result};

use super::{
    docs, runner,
    secrets::{self, Scan},
};
use crate::process::{npm, CommandSpec};

pub const ALL: &[&str] = &["secrets", "check", "docs"];

const SCAN: Scan = Scan {
    config: None,
    unpushed_without_upstream: false,
};

pub fn suite(root: &Path, name: &str) -> Result<()> {
    match name {
        "check" => runner::commands(root, [CommandSpec::new(npm()).args(["run", "check"])]),
        "docs" => docs::build_beside(root),
        "secrets" => secrets::commits(root, &SCAN),
        "staged-secrets" => secrets::staged(root, &SCAN),
        _ => bail!("Unknown suite {name}; misty-sdk has check, docs, secrets and all"),
    }
}

pub fn suites_for<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let touches = |path: &str| {
        ["packages/", "scripts/"].iter().any(|folder| path.starts_with(folder))
            || matches!(
                path,
                "package.json" | "package-lock.json" | "tsconfig.base.json" | "eslint.config.js"
            )
    };
    if paths.into_iter().any(touches) {
        vec!["check"]
    } else {
        Vec::new()
    }
}

pub fn pre_commit(root: &Path) -> Result<()> {
    runner::run("staged-secrets", || suite(root, "staged-secrets"))?;
    let staged = super::staged_paths(root)?;
    if suites_for(staged.lines()).is_empty() {
        println!("No test suites affected.");
        return Ok(());
    }
    runner::run("check", || suite(root, "check"))
}

pub fn pre_push(root: &Path) -> Result<()> {
    super::require_clean_tree(root, false)?;
    for name in ALL {
        runner::run(name, || suite(root, name))?;
    }
    Ok(())
}
