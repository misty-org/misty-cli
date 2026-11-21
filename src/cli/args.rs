//! The top-level command surface parsed by clap.

use super::*;

#[derive(Debug, Parser)]
#[command(name = "misty", version, about)]
pub struct Cli {
    /// The folder that holds the misty checkouts. Defaults to the one around the current folder, else ~/misty-org.
    #[arg(long, global = true)]
    pub workspace: Option<PathBuf>,
    /// Leave this out to be asked what to do, one step at a time.
    #[command(subcommand)]
    pub(super) command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Prepare server configuration, Cloudflare resources, or desktop dependencies.
    Setup(crate::setup::Setup),
    /// Diagnose setup and service health; choose server, desktop, cloudflare, or release.
    Doctor(crate::diagnostics::Doctor),
    /// Create and validate private runtime environments.
    Env(Env),
    /// Run a repository's checks.
    Check(Check),
    /// The git hooks' checks: install them, or run a repository's suites by hand.
    Hook(crate::hooks::Hook),
    /// Release misty-cloud or operate its production billing stack.
    Cloud(crate::cloud::Cloud),
    /// Build the app, step by step.
    Build,
    /// Run the desktop app in development, clean its build output, or update its icons or apps catalog.
    Desktop(Desktop),
    /// Run the public website, and the docs with it.
    Website(Website),
    /// Run the local server stack.
    Server(Server),
    /// Publish a release, step by step: start it, build it on each system, make it public.
    Release(Release),
}

#[derive(Debug, Args)]
pub(super) struct Check {
    #[arg(value_enum)]
    pub(super) target: CheckTarget,
}

#[derive(Debug, Args)]
pub(super) struct Env {
    #[command(subcommand)]
    pub(super) command: EnvCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum EnvCommand {
    /// Save a setting in its registered file; read the value from stdin.
    Set { name: String },
    /// List environment settings and the files that own them.
    Describe,
    /// Create missing private development files without overwriting configured values.
    Init,
    /// Validate ownership, duplicates, permissions, and required values.
    Check,
    /// Show configured counts and missing names without displaying values.
    Status,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(super) enum CheckTarget {
    Scripts,
    App,
    Server,
    GoSizes,
    Website,
    Docs,
    Features,
    Acceptance,
    Cli,
    All,
}

#[derive(Debug, Args)]
pub(super) struct Desktop {
    #[command(subcommand)]
    pub(super) command: DesktopCommand,
}

#[derive(Debug, Args)]
pub(super) struct Website {
    #[command(subcommand)]
    pub(super) command: WebsiteCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum WebsiteCommand {
    /// Start the Vite development server.
    Dev,
}

#[derive(Debug, Subcommand)]
pub(super) enum DesktopCommand {
    /// Start the app in development, always as com.misty.desktop.
    Dev {
        /// The in-app path the first window opens, e.g. /spaces/demo.
        #[arg(long)]
        route: Option<String>,
    },
    /// List the build output and caches in misty that can be removed.
    Clean {
        /// Remove them.
        #[arg(long)]
        apply: bool,
    },
    /// Update the app's icons.
    Icons {
        #[command(subcommand)]
        command: IconCommand,
    },
    /// Update the apps catalog the app carries.
    Apps {
        #[command(subcommand)]
        command: AppsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum AppsCommand {
    /// Copy misty-apps' catalog into the app.
    Sync {
        /// The misty-apps checkout to copy from. Defaults to the one beside misty.
        #[arg(long)]
        source: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum IconCommand {
    /// Make every icon size from one .icns file.
    Sync {
        /// The .icns to use. Defaults to src-tauri/icons/icon.icns.
        #[arg(long)]
        source: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
pub(super) struct Release {
    /// Walk through the steps without tagging, uploading or publishing anything.
    #[arg(long)]
    pub(super) dry_run: bool,
}
