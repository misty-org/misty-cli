use anyhow::Result;
use std::path::Path;

use crate::{
    process::{npm, CommandSpec},
    workspace::Workspace,
};

/// Runs the website. The website mounts misty-docs on its docs host (`docs.localhost`), so this is the one
/// command for both; a docs checkout beside it only needs its packages installed first, which happens here.
pub fn dev(workspace: &Workspace) -> Result<()> {
    workspace.validate()?;
    let docs = workspace.docs();
    if docs.join("package.json").is_file() {
        if needs_install(&docs) {
            CommandSpec::new(npm())
                .args(["install", "--no-audit", "--no-fund"])
                .run(&docs)?;
        }
        eprintln!("Docs: misty-docs is mounted in the website, on docs.localhost.");
    } else {
        eprintln!(
            "Docs: no misty-docs beside this checkout, so the docs are left out. Clone it with: git clone https://github.com/misty-org/misty-docs.git"
        );
    }
    dev_command().run(&workspace.website)
}

fn dev_command() -> CommandSpec {
    CommandSpec::new(npm()).args(["run", "dev"])
}

/// Whether a checkout's npm packages have never been installed.
fn needs_install(checkout: &Path) -> bool {
    !checkout.join("node_modules").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_uses_the_website_workspace_script() {
        let expected = if cfg!(windows) {
            "npm.cmd run dev"
        } else {
            "npm run dev"
        };
        assert_eq!(dev_command().display(), expected);
    }

    #[test]
    fn a_checkout_without_node_modules_needs_its_packages() {
        let folder = std::env::temp_dir().join(format!("misty-cli-docs-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        assert!(needs_install(&folder));
        std::fs::create_dir_all(folder.join("node_modules")).unwrap();
        assert!(!needs_install(&folder));
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
