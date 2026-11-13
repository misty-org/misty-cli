use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub misty: PathBuf,
    pub server: PathBuf,
    pub website: PathBuf,
    pub features: PathBuf,
    pub cli: PathBuf,
    pub cloud: PathBuf,
}

impl Workspace {
    /// The misty checkout at or above `start`, or beside one of its folders.
    pub fn discover(start: &Path) -> Option<PathBuf> {
        let is_app = |path: &Path| path.join("src-tauri/tauri.conf.json").is_file();
        start.ancestors().find_map(|path| {
            if is_app(path) {
                return Some(path.to_path_buf());
            }
            let sibling = path.join("misty");
            is_app(&sibling).then_some(sibling)
        })
    }

    pub fn from_root(root: PathBuf) -> Result<Self> {
        let absolute = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .context("could not read current directory")?
                .join(root)
        };
        let misty = Self::discover(&absolute).unwrap_or_else(|| absolute.join("misty"));
        let root = misty.parent().unwrap_or(&misty).to_path_buf();
        // A linked worktree sits inside the checkout, so look up from it for the siblings.
        let sibling = |name: &str, marker: &str| {
            misty
                .ancestors()
                .map(|folder| folder.join(name))
                .find(|folder| folder.join(marker).is_file())
                .unwrap_or_else(|| root.join(name))
        };
        Ok(Self {
            features: misty.join("src/features"),
            cli: sibling("misty-cli", "Cargo.toml"),
            server: sibling("misty-server", "go.mod"),
            website: root.join("misty-website"),
            cloud: root.join("misty-cloud"),
            misty,
            root,
        })
    }

    /// For commands that work on the server, which is a sibling checkout.
    pub fn require_server(&self) -> Result<()> {
        if !self.server.join("go.mod").is_file() {
            bail!(
                "This needs misty-server beside this checkout, at {}. Clone it with: git clone https://github.com/misty-org/misty-server.git",
                self.server.display()
            );
        }
        Ok(())
    }

    /// Local development needs only the product checkout. Remote services and
    /// the website are optional siblings, validated by their own commands.
    pub fn validate(&self) -> Result<()> {
        for (path, label) in [
            (self.misty.join("package.json"), "Misty package.json"),
            (
                self.misty.join("src-tauri/tauri.conf.json"),
                "Misty native configuration",
            ),
            (self.cli.join("Cargo.toml"), "misty-cli"),
        ] {
            if !path.is_file() {
                bail!("{label} was not found at {}", path.display());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(root: &Path, names: &[&str]) {
        for name in names {
            let file = root.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "").unwrap();
        }
    }

    #[test]
    fn finds_the_app_from_its_own_folders_and_from_misty_cli() {
        let temporary = tempfile::tempdir().unwrap();
        let checkout = temporary.path().join("misty");
        let cli = temporary.path().join("misty-cli");
        touch(
            &checkout,
            &[
                "package.json",
                "src-tauri/tauri.conf.json",
                "src/shared/schemas/index.ts",
            ],
        );
        touch(&cli, &["Cargo.toml", "src/main.rs"]);
        for path in [
            checkout.clone(),
            checkout.join("src/shared/schemas"),
            cli.clone(),
            cli.join("src"),
        ] {
            let workspace = Workspace::from_root(path).unwrap();
            workspace.validate().unwrap();
            assert_eq!(workspace.misty, checkout);
            assert_eq!(workspace.features, checkout.join("src/features"));
            assert_eq!(workspace.cli, cli);
            assert!(!workspace.server.exists());
        }
    }

    #[test]
    fn organization_root_resolves_the_product_checkout() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_root(temporary.path().to_path_buf()).unwrap();
        assert_eq!(workspace.misty, temporary.path().join("misty"));
        assert_eq!(workspace.server, temporary.path().join("misty-server"));
        assert_eq!(workspace.cli, temporary.path().join("misty-cli"));
        assert!(workspace.require_server().is_err());
    }

    #[test]
    fn a_linked_worktree_finds_the_siblings_beside_the_main_checkout() {
        let temporary = tempfile::tempdir().unwrap();
        let worktree = temporary.path().join("misty/.worktrees/feature");
        touch(&worktree, &["src-tauri/tauri.conf.json"]);
        touch(
            temporary.path(),
            &["misty-server/go.mod", "misty-cli/Cargo.toml"],
        );
        let workspace = Workspace::from_root(worktree).unwrap();
        assert_eq!(workspace.server, temporary.path().join("misty-server"));
        assert_eq!(workspace.cli, temporary.path().join("misty-cli"));
        workspace.require_server().unwrap();
    }
}
