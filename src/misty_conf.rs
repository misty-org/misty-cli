//! `~/.misty/misty.conf`, as the desktop app writes it (`file_access/conf.rs` in misty): which data
//! folder is in use (`[metadata] misty.root`, always `~/.misty/dev/<kind>` or
//! `~/.misty/rel/<version>`), and environment defaults for each program (`[server]`, `[cli]`),
//! used only where neither the shell nor a `.env` file sets the name. It never holds secrets.
//!
//! The development server's settings live in that folder's `.env/server`, so `misty setup server`,
//! the dev stack and goose all follow the folder `misty.conf` names.
use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

/// `~/.misty`.
fn misty_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".misty"))
}

struct Conf {
    root: Option<String>,
    sections: Vec<(String, String, String)>,
}

fn read() -> Option<Conf> {
    let text = fs::read_to_string(misty_root()?.join("misty.conf")).ok()?;
    let mut conf = Conf {
        root: None,
        sections: Vec::new(),
    };
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            section = name.trim().to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), unquote(value.trim()));
        if section == "metadata" && key == "misty.root" {
            conf.root = Some(value.to_owned());
        } else if !key.is_empty() {
            conf.sections
                .push((section.clone(), key.to_owned(), value.to_owned()));
        }
    }
    Some(conf)
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// The data folder `misty.root` names when it is one Misty allows, else `~/.misty/dev/default`.
pub fn data_folder() -> PathBuf {
    let Some(misty) = misty_root() else {
        return PathBuf::from(".misty/dev/default");
    };
    let fallback = misty.join("dev").join("default");
    let Some(root) = read().and_then(|conf| conf.root) else {
        return fallback;
    };
    let full = match root.strip_prefix("~/") {
        Some(rest) => misty.parent().map_or(PathBuf::from(rest), |home| home.join(rest)),
        None => PathBuf::from(&root),
    };
    allowed(&misty, &full).then_some(full).unwrap_or(fallback)
}

/// `~/.misty/dev/<kind>` or `~/.misty/rel/<version>` exactly, with no `..` and no link on the way.
fn allowed(misty: &Path, folder: &Path) -> bool {
    let Ok(rest) = folder.strip_prefix(misty) else {
        return false;
    };
    let parts: Vec<_> = rest.components().collect();
    let [Component::Normal(kind), Component::Normal(name)] = parts.as_slice() else {
        return false;
    };
    let name = name.to_string_lossy();
    (*kind == "dev" || *kind == "rel")
        && !name.is_empty()
        && !name.chars().all(|c| c == '.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && [misty.join(kind), folder.to_path_buf()]
            .iter()
            .all(|step| !fs::symlink_metadata(step).is_ok_and(|meta| meta.file_type().is_symlink()))
}

/// The development server's settings folder: `MISTY_ENV_DIR`, else the data folder's `.env/server`.
pub fn server_env_dir() -> PathBuf {
    env::var_os("MISTY_ENV_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_folder().join(".env").join("server"))
}

/// Whether an environment name reads like a secret, which misty.conf must never hold.
fn looks_secret(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    ["_KEY", "_TOKEN", "_SECRET", "_PASSWORD", "_PASS"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
        || ["PRIVATE", "SECRET", "PASSWORD"]
            .iter()
            .any(|word| name.contains(word))
}

/// Sets each name in `section` (`server` or `cli`) this process doesn't have yet. Secret-looking
/// names are skipped: they belong in `.env`.
pub fn apply_defaults(section: &str) {
    let Some(conf) = read() else {
        return;
    };
    for (_, name, value) in conf.sections.iter().filter(|(found, _, _)| found == section) {
        if looks_secret(name) {
            eprintln!("misty.conf: [{section}] {name} looks like a secret; keep it in .env");
            continue;
        }
        if env::var_os(name).is_none() {
            env::set_var(name, value);
        }
    }
}

/// Points `MISTY_ENV_DIR` at the data folder's `.env/server` unless the shell set it, so Compose
/// and every server command use the same folder.
pub fn point_server_env_dir() {
    if env::var_os("MISTY_ENV_DIR").is_none() {
        env::set_var("MISTY_ENV_DIR", server_env_dir());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dev_and_release_folders_in_misty_are_allowed() {
        let misty = Path::new("/home/me/.misty");
        assert!(allowed(misty, &misty.join("dev/default")));
        assert!(allowed(misty, &misty.join("rel/0.2.0")));
        for outside in ["dev", "shared/engines", "dev/../x", "dev/a/b"] {
            assert!(!allowed(misty, &misty.join(outside)), "{outside}");
        }
        assert!(!allowed(misty, Path::new("/tmp/.misty/dev/a")));
        assert!(looks_secret("GOOGLE_CLIENT_SECRET"));
        assert!(looks_secret("OPENAI_API_KEY"));
        assert!(!looks_secret("DB_NAME"));
    }
}
