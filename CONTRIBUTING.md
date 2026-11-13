# Contributing to misty-cli

The CLI is one Rust crate. It never builds against misty's source: it finds the sibling checkouts
at run time (`src/workspace.rs`) and drives them through their own npm scripts, cargo and Docker.

Before you open a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
```

`misty check cli` runs the same three.

Write commit subjects as plain sentences that say what changed. By opening a pull request you agree
that your contribution is licensed under the [AGPL-3.0-or-later](LICENSE) with the terms in
[NOTICE](NOTICE).

Report vulnerabilities privately, as misty's
[SECURITY.md](https://github.com/misty-org/misty/blob/main/SECURITY.md) describes.
