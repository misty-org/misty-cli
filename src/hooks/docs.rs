//! misty-docs's checks: a commit type-checks when its staged changes touch the site; a push runs a
//! production build on a clean tree, which also reads the docs of every repository it pulls from.

use std::path::Path;

use anyhow::{bail, Result};

use super::{
    runner,
    secrets::{self, Scan},
};
use crate::process::{npm, CommandSpec};

pub const ALL: &[&str] = &["secrets", "build"];

const SCAN: Scan = Scan {
    config: None,
    unpushed_without_upstream: false,
};

pub fn suite(root: &Path, name: &str) -> Result<()> {
    match name {
        "typecheck" => runner::commands(root, [CommandSpec::new(npm()).args(["run", "typecheck"])]),
        // `tsc -b && vite build`, reading each source in sources.json.
        "build" => runner::commands(root, [CommandSpec::new(npm()).args(["run", "build"])]),
        "secrets" => secrets::commits(root, &SCAN),
        "staged-secrets" => secrets::staged(root, &SCAN),
        _ => bail!("Unknown suite {name}; misty-docs has typecheck, build, secrets and all"),
    }
}

/// Builds the docs site beside `root`, if there is one, so a change to a repository it reads (misty-sdk,
/// misty-ui) is checked against the pages it would break. A computer without misty-docs skips it.
pub fn build_beside(root: &Path) -> Result<()> {
    let Some(docs) = root.parent().map(|parent| parent.join("misty-docs")) else {
        return Ok(());
    };
    if !docs.join("package.json").is_file() {
        println!("No misty-docs beside this checkout; the docs build is skipped.");
        return Ok(());
    }
    runner::commands(&docs, [CommandSpec::new(npm()).args(["run", "build"])])
}

pub fn suites_for<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let touches = |path: &str| {
        ["src/", "content/", "scripts/"]
            .iter()
            .any(|folder| path.starts_with(folder))
            || matches!(
                path,
                "package.json"
                    | "package-lock.json"
                    | "sources.json"
                    | "vite.config.ts"
                    | "index.html"
            )
            || (path.starts_with("tsconfig") && path.ends_with(".json"))
    };
    if paths.into_iter().any(touches) {
        vec!["typecheck"]
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
    runner::run("typecheck", || suite(root, "typecheck"))
}

pub fn pre_push(root: &Path) -> Result<()> {
    super::require_clean_tree(root, false)?;
    for name in ALL {
        runner::run(name, || suite(root, name))?;
    }
    Ok(())
}
