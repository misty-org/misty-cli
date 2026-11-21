//! The local checks behind every repository's git hooks (there is no hosted CI).
//!
//! `misty hook install` writes a two-line `pre-commit` and `pre-push` into each checkout's own
//! git hooks folder; they call back into this module, which works out which repository it is in
//! and runs that repository's suites. Nothing is tracked in the repositories themselves.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

use crate::{process::CommandSpec, workspace::Workspace};

mod app;
mod cloud;
mod docs;
mod postgres;
mod runner;
mod secrets;
mod server;
mod sdk;
mod server_tests;
#[cfg(test)]
mod tests;
mod ui;
mod website;

pub use runner::Failed;
pub(crate) use server_tests::run as run_isolated_server_tests;

#[derive(Debug, Args)]
pub struct Hook {
    #[command(subcommand)]
    pub command: HookCommand,
}

#[derive(Debug, Subcommand)]
pub enum HookCommand {
    /// Install pre-commit and pre-push into every checkout of the workspace.
    Install,
    /// What git runs before a commit: the suites the staged changes touch.
    PreCommit,
    /// What git runs before a push; git passes the pushed refs on stdin.
    PrePush {
        remote: Option<String>,
        url: Option<String>,
    },
    /// Run suites by name in the repository around the current folder, e.g. `frontend` or `all`.
    Run {
        #[arg(required = true)]
        suites: Vec<String>,
    },
    /// Print the suites a list of changed paths (one per line on stdin) would run.
    Plan,
}

/// The repositories that have hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repo {
    App,
    Server,
    Cloud,
    Website,
    Docs,
    Sdk,
    Ui,
}

impl Repo {
    pub fn detect(root: &Path) -> Result<Self> {
        if root.join("src-tauri/tauri.conf.json").is_file() {
            Ok(Self::App)
        } else if root.join("cmd/misty-server").is_dir() {
            Ok(Self::Server)
        } else if root.join("cmd/misty-cloud").is_dir() {
            Ok(Self::Cloud)
        } else if root.join("sources.json").is_file() && root.join("vite.config.ts").is_file() {
            // Before the website, which has the same Vite files.
            Ok(Self::Docs)
        } else if root.join("vite.config.ts").is_file() && root.join("index.html").is_file() {
            Ok(Self::Website)
        } else if root.join("packages/sdk/package.json").is_file() {
            Ok(Self::Sdk)
        } else if root.join("DESIGN.md").is_file() && root.join("src/index.ts").is_file() {
            Ok(Self::Ui)
        } else {
            bail!(
                "{} is not misty, misty-server, misty-cloud, misty-website, misty-docs, misty-sdk or misty-ui",
                root.display()
            )
        }
    }

    /// Runs one named suite in this process.
    pub fn suite(self, root: &Path, name: &str) -> Result<()> {
        match self {
            Self::App => app::suite(root, name),
            Self::Server => server::suite(root, name),
            Self::Cloud => cloud::suite(root, name),
            Self::Website => website::suite(root, name),
            Self::Docs => docs::suite(root, name),
            Self::Sdk => sdk::suite(root, name),
            Self::Ui => ui::suite(root, name),
        }
    }

    /// Runs one named suite in this process, framed by its name and how long it took.
    pub fn run(self, root: &Path, name: &str) -> Result<()> {
        runner::run(name, || self.suite(root, name))
    }

    /// The suites `misty hook run all` stands for.
    fn expand<'a>(self, name: &'a str) -> Vec<&'a str> {
        match (self, name) {
            (Self::App, "all") => app::ALL.to_vec(),
            (Self::App, "release") => app::RELEASE.to_vec(),
            (Self::Server, "all") => server::ALL.to_vec(),
            (Self::Cloud, "all") => cloud::ALL.to_vec(),
            (Self::Website, "all") => website::ALL.to_vec(),
            (Self::Docs, "all") => docs::ALL.to_vec(),
            (Self::Sdk, "all") => sdk::ALL.to_vec(),
            (Self::Ui, "all") => ui::ALL.to_vec(),
            _ => vec![name],
        }
    }
}

pub fn dispatch(workspace: &Workspace, command: HookCommand) -> Result<()> {
    if let HookCommand::Install = command {
        return install(workspace);
    }
    // Git exports GIT_DIR and friends to hooks (all of them in a linked worktree). Tests that
    // create their own repositories must not inherit them, or their git init and config would
    // write to this repository. From the checkout root, git finds the same repository without them.
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_PREFIX",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        std::env::remove_var(name);
    }
    let root = toplevel(&std::env::current_dir()?)?;
    let repo = Repo::detect(&root)?;
    let result = match command {
        HookCommand::Install => unreachable!(),
        HookCommand::PreCommit => match repo {
            Repo::App => app::pre_commit(&root),
            Repo::Server => server::pre_commit(&root),
            Repo::Cloud => cloud::pre_commit(&root),
            Repo::Website => website::pre_commit(&root),
            Repo::Docs => docs::pre_commit(&root),
            Repo::Sdk => sdk::pre_commit(&root),
            Repo::Ui => ui::pre_commit(&root),
        },
        HookCommand::PrePush { .. } => {
            let mut refs = String::new();
            std::io::stdin().read_to_string(&mut refs)?;
            match repo {
                Repo::App => app::pre_push(&root, &refs),
                Repo::Server => server::pre_push(&root, &refs),
                Repo::Cloud => cloud::pre_push(&root),
                Repo::Website => website::pre_push(&root),
                Repo::Docs => docs::pre_push(&root),
                Repo::Sdk => sdk::pre_push(&root),
                Repo::Ui => ui::pre_push(&root),
            }
        }
        HookCommand::Run { suites } => {
            let names: Vec<&str> = suites.iter().flat_map(|name| repo.expand(name)).collect();
            if let [name] = names.as_slice() {
                repo.run(&root, name)
            } else {
                runner::sequence(&root, 0, &names)
            }
        }
        HookCommand::Plan => {
            let mut paths = String::new();
            std::io::stdin().read_to_string(&mut paths)?;
            let suites = match repo {
                Repo::App => app::suites_for(paths.lines()),
                Repo::Server => server::suites_for(paths.lines()),
                Repo::Cloud => cloud::suites_for(paths.lines()),
                Repo::Website => website::suites_for(paths.lines()),
                Repo::Docs => docs::suites_for(paths.lines()),
                Repo::Sdk => sdk::suites_for(paths.lines()),
                Repo::Ui => ui::suites_for(paths.lines()),
            };
            for suite in suites {
                println!("{suite}");
            }
            Ok(())
        }
    };
    if result.is_err() {
        eprintln!("Fix it, or skip once with --no-verify.");
    }
    result
}

/// Writes the two hooks into every checkout beside this one and drops any older
/// `core.hooksPath`, which would hide them.
fn install(workspace: &Workspace) -> Result<()> {
    let program = std::env::current_exe().context("could not find the misty binary")?;
    let mut installed = 0;
    for checkout in [
        workspace.misty.clone(),
        workspace.server.clone(),
        workspace.cloud.clone(),
        workspace.website.clone(),
        workspace.docs(),
        workspace.sdk(),
        workspace.ui(),
    ] {
        let checkout = &checkout;
        if !checkout.join(".git").exists() {
            continue;
        }
        let _ = CommandSpec::new("git")
            .args(["config", "--local", "--unset", "core.hooksPath"])
            .succeeds(checkout);
        let hooks = CommandSpec::new("git")
            .args(["rev-parse", "--git-path", "hooks"])
            .capture(checkout)?;
        let hooks = checkout.join(hooks.trim());
        fs::create_dir_all(&hooks)?;
        for (hook, arguments) in [("pre-commit", ""), ("pre-push", " \"$@\"")] {
            let path = hooks.join(hook);
            fs::write(&path, hook_script(&program, hook, arguments))?;
            make_executable(&path)?;
        }
        println!("Installed pre-commit and pre-push in {}", checkout.display());
        installed += 1;
    }
    if installed == 0 {
        bail!(
            "No checkouts found beside {}",
            workspace.misty.display()
        );
    }
    Ok(())
}

fn hook_script(program: &Path, hook: &str, arguments: &str) -> String {
    format!(
        "#!/bin/sh\n# Installed by `misty hook install`; the checks live in misty-cli.\nexec '{}' hook {hook}{arguments}\n",
        program.display().to_string().replace('\'', "'\\''")
    )
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<()> {
    Ok(())
}

pub(crate) fn toplevel(directory: &Path) -> Result<PathBuf> {
    let root = CommandSpec::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .capture(directory)
        .context("not inside a git checkout")?;
    Ok(PathBuf::from(root.trim()))
}

pub(crate) fn git(root: &Path, arguments: &[&str]) -> Result<String> {
    CommandSpec::new("git").args(arguments).capture(root)
}

/// The paths a commit would add, change, rename or delete.
fn staged_paths(root: &Path) -> Result<String> {
    git(root, &["diff", "--cached", "--name-only", "--diff-filter=ACMRD"])
}

/// Tests run on the working tree, so before a commit it must hold exactly what is staged.
fn require_staged_snapshot(root: &Path) -> Result<()> {
    let unstaged = !CommandSpec::new("git")
        .args(["diff", "--quiet"])
        .succeeds(root);
    if unstaged || !git(root, &["ls-files", "--others", "--exclude-standard"])?.trim().is_empty() {
        bail!("Stage or stash outstanding files before committing so tests match the staged snapshot.");
    }
    Ok(())
}

/// Before a push, the working tree must match what is being pushed.
fn require_clean_tree(root: &Path, untracked: bool) -> Result<()> {
    let changed = !CommandSpec::new("git")
        .args(["diff", "--quiet"])
        .succeeds(root)
        || !CommandSpec::new("git")
            .args(["diff", "--cached", "--quiet"])
            .succeeds(root);
    let stray = untracked
        && !git(root, &["ls-files", "--others", "--exclude-standard"])?
            .trim()
            .is_empty();
    if changed || stray {
        bail!("Commit or discard local changes before pushing; checks must run on the pushed commit.");
    }
    Ok(())
}

/// Refuses pushes that would publish history the public repository must not contain.
fn check_public_history(root: &Path, script: &str, refs: &str) -> Result<()> {
    for line in refs.lines() {
        let mut fields = line.split_whitespace();
        let (Some(_), Some(local_sha)) = (fields.next(), fields.next()) else {
            continue;
        };
        if local_sha.chars().all(|c| c == '0') {
            continue;
        }
        CommandSpec::new("python3")
            .arg(root.join(script).as_os_str())
            .arg(local_sha)
            .run(root)?;
    }
    Ok(())
}
