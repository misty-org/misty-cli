//! misty-ui's checks: its tests (which include the contract tests that keep the catalog current and
//! focus rings out) when a commit touches the package; a push adds the type check and the docs
//! build, since the docs site draws every component from this repository.

use std::path::Path;

use anyhow::{bail, Result};

use super::{
    docs, runner,
    secrets::{self, Scan},
};
use crate::process::{npm, CommandSpec};

pub const ALL: &[&str] = &["secrets", "typecheck", "test", "docs"];

const SCAN: Scan = Scan {
    config: None,
    unpushed_without_upstream: false,
};

pub fn suite(root: &Path, name: &str) -> Result<()> {
    match name {
        "typecheck" => runner::commands(root, [CommandSpec::new(npm()).args(["run", "typecheck"])]),
        "test" => runner::commands(root, [CommandSpec::new(npm()).args(["test"])]),
        "docs" => docs::build_beside(root),
        "secrets" => secrets::commits(root, &SCAN),
        "staged-secrets" => secrets::staged(root, &SCAN),
        _ => bail!("Unknown suite {name}; misty-ui has typecheck, test, docs, secrets and all"),
    }
}

pub fn suites_for<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let touches = |path: &str| {
        ["src/", "styles/", "site/", "tests/", "scripts/", "docs/"]
            .iter()
            .any(|folder| path.starts_with(folder))
            || matches!(
                path,
                "DESIGN.md"
                    | "package.json"
                    | "package-lock.json"
                    | "tsconfig.json"
                    | "vitest.config.ts"
            )
    };
    if paths.into_iter().any(touches) {
        vec!["test"]
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
    runner::run("test", || suite(root, "test"))
}

pub fn pre_push(root: &Path) -> Result<()> {
    super::require_clean_tree(root, false)?;
    for name in ALL {
        runner::run(name, || suite(root, name))?;
    }
    Ok(())
}
