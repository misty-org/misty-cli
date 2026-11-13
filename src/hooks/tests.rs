use super::*;

#[test]
fn misty_change_selection_covers_configuration_helper_crates_and_the_dev_runner() {
    let plan = |paths: &[&str]| app::suites_for(paths.iter().copied());
    assert!(plan(&["README.md"]).is_empty());
    assert_eq!(plan(&["src/features/example.ts"]), ["scripts", "frontend"]);
    assert_eq!(plan(&["tsconfig.app.json"]), ["scripts", "frontend"]);
    assert_eq!(
        plan(&["src-tauri/features/documents/worker/Cargo.toml"]),
        ["rust-desktop"]
    );
    assert_eq!(plan(&["rust-toolchain.toml"]), ["rust-desktop"]);
    assert_eq!(plan(&["tasks/tauri.ts"]), ["scripts"]);
    assert_eq!(
        plan(&["src/a.ts", "src-tauri/b.rs"]),
        ["scripts", "rust-desktop", "frontend"]
    );
}

#[test]
fn server_cloud_and_website_select_their_suites() {
    assert_eq!(
        server::suites_for(["services/journal-collab/src/a.ts", "internal/x.go"]),
        ["collab", "go"]
    );
    assert!(server::suites_for(["README.md"]).is_empty());
    assert_eq!(cloud::suites_for(["deploy/runtime-grants.sql"]), ["go"]);
    assert!(cloud::suites_for(["README.md"]).is_empty());
    assert_eq!(website::suites_for(["index.html"]), ["web"]);
    assert_eq!(website::suites_for(["tsconfig.node.json"]), ["web"]);
    assert!(website::suites_for(["README.md"]).is_empty());
}

#[test]
fn all_expands_per_repository() {
    assert_eq!(Repo::App.expand("all"), app::ALL);
    assert_eq!(Repo::App.expand("release").last(), Some(&"desktop-windows"));
    assert_eq!(Repo::Server.expand("collab"), ["collab"]);
}

#[test]
fn repositories_are_told_apart_by_their_layout() {
    let directory = tempfile::tempdir().unwrap();
    let make = |name: &str, files: &[&str]| {
        let root = directory.path().join(name);
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        root
    };
    let app = make("misty", &["src-tauri/tauri.conf.json"]);
    let server = make("misty-server", &["cmd/misty-server/main.go"]);
    let cloud = make("misty-cloud", &["cmd/misty-cloud/main.go"]);
    let website = make("misty-website", &["vite.config.ts", "index.html"]);
    assert_eq!(Repo::detect(&app).unwrap(), Repo::App);
    assert_eq!(Repo::detect(&server).unwrap(), Repo::Server);
    assert_eq!(Repo::detect(&cloud).unwrap(), Repo::Cloud);
    assert_eq!(Repo::detect(&website).unwrap(), Repo::Website);
    assert!(Repo::detect(directory.path()).is_err());
}

#[test]
fn installed_hooks_call_back_into_this_binary() {
    let script = hook_script(Path::new("/opt/it's/misty"), "pre-push", " \"$@\"");
    assert!(script.starts_with("#!/bin/sh\n"));
    assert!(script.ends_with("exec '/opt/it'\\''s/misty' hook pre-push \"$@\"\n"));
}

#[test]
fn a_commit_refuses_to_validate_unstaged_code_instead_of_the_staged_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let git = |arguments: &[&str]| {
        CommandSpec::new("git")
            .args(arguments)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .capture(root)
            .unwrap();
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.invalid"]);
    fs::write(root.join("fixture"), "original").unwrap();
    git(&["add", "fixture"]);
    git(&["-c", "core.hooksPath=/dev/null", "commit", "-qm", "fixture"]);
    fs::write(root.join("fixture"), "staged").unwrap();
    git(&["add", "fixture"]);
    assert!(require_staged_snapshot(root).is_ok());
    fs::write(root.join("fixture"), "different working tree").unwrap();
    let error = require_staged_snapshot(root).unwrap_err().to_string();
    assert!(error.contains("tests match the staged snapshot"));
}
