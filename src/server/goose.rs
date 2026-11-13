//! `misty server goose <command>`: goose on the server's migrations, with the development
//! database's connection unless the caller supplies one.

use std::{env, path::Path};

use anyhow::{bail, Result};

use crate::process::{command_exists, CommandSpec};

const SETTINGS: &[&str] = &[
    "DB_HOST",
    "DB_PORT",
    "DB_USER",
    "DB_PASSWORD",
    "DB_NAME",
    "DB_SSLMODE",
    "DB_MIGRATION_USER",
    "DB_MIGRATION_PASSWORD",
    "MIGRATIONS_DIR",
];

pub fn goose(server: &Path, arguments: &[String]) -> Result<()> {
    if arguments.is_empty() {
        bail!("Name a goose command, such as: misty server goose status | up | down");
    }
    // A caller-supplied database setting means the entire connection contract comes from the
    // caller. Never mix an isolated CI/staging connection with migration credentials silently
    // loaded from a developer's normal database env file.
    if !SETTINGS.iter().any(|name| env::var_os(name).is_some()) {
        let directory = env::var_os("MISTY_ENV_DIR")
            .map(Into::into)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".misty/.env/server"));
        if let Ok(values) = dotenvy::from_path_iter(directory.join("database.env")) {
            for (name, value) in values.flatten() {
                env::set_var(name, value);
            }
        }
    }
    let get = |name: &str, default: &str| {
        env::var(name)
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| default.to_string())
    };
    let host = get("DB_HOST", "localhost");
    let port = get("DB_PORT", "5432");
    let user = get("DB_USER", "misty");
    let password = get("DB_PASSWORD", "misty");
    let name = get("DB_NAME", "misty_server");
    let migration_user = get("DB_MIGRATION_USER", &user);
    let migration_password = get("DB_MIGRATION_PASSWORD", &password);
    let local = matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1" | "postgres")
        || host.starts_with('/');
    let sslmode = get("DB_SSLMODE", if local { "disable" } else { "require" });
    if !matches!(
        sslmode.as_str(),
        "disable" | "allow" | "prefer" | "require" | "verify-ca" | "verify-full"
    ) {
        bail!("DB_SSLMODE must be one of disable, allow, prefer, require, verify-ca, or verify-full.");
    }
    let migrations = get("MIGRATIONS_DIR", "internal/platform/db/migrations");
    if !command_exists("goose") {
        bail!("goose is not installed. Install it with:\n  go install github.com/pressly/goose/v3/cmd/goose@latest");
    }
    let dsn = format!(
        "host={host} port={port} user={migration_user} password={migration_password} dbname={name} sslmode={sslmode}"
    );
    // Run without echoing the command line, which holds the password.
    let status = CommandSpec::new("goose")
        .args(["-dir", migrations.as_str(), "postgres", dsn.as_str()])
        .args(arguments)
        .command(server)
        .status()?;
    let code = status.code().unwrap_or(1);
    if code != 0 {
        return Err(crate::hooks::Failed(code).into());
    }
    Ok(())
}
