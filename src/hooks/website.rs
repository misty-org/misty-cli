//! misty-website's checks: a commit runs the web suite when its staged changes touch the site; a
//! push runs everything on a clean tree.

use std::path::Path;

use anyhow::{bail, Result};

use super::{
    runner,
    secrets::{self, Scan},
};
use crate::process::{npm, CommandSpec};

pub const ALL: &[&str] = &["secrets", "web", "build"];

const SCAN: Scan = Scan {
    config: None,
    unpushed_without_upstream: false,
};

pub fn suite(root: &Path, name: &str) -> Result<()> {
    match name {
        "web" => runner::commands(
            root,
            ["typecheck", "lint", "test"].map(|script| CommandSpec::new(npm()).args(["run", script])),
        ),
        // The production bundle, which calls api.mistysys.com unless told otherwise.
        "build" => runner::commands(
            root,
            [CommandSpec::new(npm())
                .args(["run", "build"])
                .env("NODE_OPTIONS", "--max-old-space-size=4096")],
        ),
        "secrets" => secrets::commits(root, &SCAN),
        "staged-secrets" => secrets::staged(root, &SCAN),
        _ => bail!("Unknown suite {name}; misty-website has web, build, secrets and all"),
    }
}

pub fn suites_for<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let touches = |path: &str| {
        ["src/", "tests/", "public/", "scripts/"]
            .iter()
            .any(|folder| path.starts_with(folder))
            || matches!(
                path,
                "package.json"
                    | "package-lock.json"
                    | "vite.config.ts"
                    | "vitest.config.ts"
                    | "eslint.config.js"
                    | "index.html"
            )
            || (path.starts_with("tsconfig") && path.ends_with(".json"))
    };
    if paths.into_iter().any(touches) {
        vec!["web"]
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
    runner::run("web", || suite(root, "web"))
}

pub fn pre_push(root: &Path) -> Result<()> {
    super::require_clean_tree(root, false)?;
    for name in ALL {
        runner::run(name, || suite(root, name))?;
    }
    Ok(())
}
