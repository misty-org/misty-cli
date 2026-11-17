# misty-cli

The `misty` command: development and release tooling for [Misty](https://github.com/misty-org/misty).
It sets up a computer, runs the local server stack, runs every repository's checks and publishes
releases. The desktop app doesn't need it; `npm run tauri dev` in misty is enough to work on the app.

## Install

Check it out beside the other repositories, where it finds them:

```
misty-org/
  misty/
  misty-cli/
  misty-server/    optional, for the server commands
  misty-cloud/     optional, for the cloud commands
  misty-website/   optional, for the website command
```

```sh
cd misty-cli
cargo install --path . --locked
```

Then turn on the git hooks in every checkout beside it with `misty hook install` (again after
moving the binary, since the hooks call it by path).

Or run it in place with `cargo run --`. With no command, `misty` asks what you want to do, one
step at a time. `--workspace <folder>` or `MISTY_ROOT` points it at another folder of checkouts.

## Commands

| Command | What it does |
| --- | --- |
| `misty setup [all\|server\|desktop\|cloudflare]` | Prepares this computer: misty's npm packages, the server's development keys, the Cloudflare tunnel and DNS. |
| `misty doctor [all\|server\|desktop\|cloudflare\|release]` | Checks tools, settings and service health. |
| `misty env init\|check\|status\|describe\|set <NAME>` | Manages the private settings files. `set` reads the value from stdin. |
| `misty desktop dev [--route /path]` | Runs the app against the local server stack. |
| `misty desktop clean [--apply]` | Lists misty's build output and caches; `--apply` removes them. |
| `misty desktop icons sync` | Makes every icon size from `src-tauri/icons/icon.icns`. |
| `misty server up\|status\|logs\|down\|url\|deploy` | Runs the development stack: Postgres, the API and a Cloudflare tunnel in Docker, and the collaboration Worker on Cloudflare. |
| `misty server test [--isolated] [go test args]` | Runs the Go suite on a fresh test database in the development stack's Postgres, or with `--isolated` on a disposable container, split across jobs, as the hooks do. |
| `misty server goose <command>` | Runs goose on the server's migrations against the development database, e.g. `up` or `status`. |
| `misty server stack-test` | Runs the whole server stack (`compose.yml`) against a fake Cloudflare API, then removes everything it made. |
| `misty website dev` | Runs the website. |
| `misty check app\|scripts\|features\|server\|go-sizes\|website\|acceptance\|cli\|all` | Runs a repository's checks. `acceptance` runs misty's live tests against real models and the local stack. |
| `misty hook install` | Installs `pre-commit` and `pre-push` in misty, misty-server, misty-cloud and misty-website. There is no hosted CI. |
| `misty hook run <suite>…` | Runs the current repository's hook suites by hand, e.g. `frontend`, `go`, `web`, `secrets` or `all`. `MISTY_COMMIT_SECONDS=<n>` caps misty's commit checks for once. |
| `misty cloud release <version>` | Builds misty-cloud's billing image here, pushes it to GHCR, tags the commit and pins the digest in `deploy/billing/version.env` (commit it). |
| `misty cloud pin-server <commit>` | Pins the misty-server release production runs: its `compose.yml` and published images, in misty-cloud's `deploy/misty-server/` (commit both). |
| `misty cloud ship [--host misty-vps] [--path misty-cloud] [--up]` | Copies the committed deploy files (`deploy/prod-files.txt`) to the VPS, never `.env/`; `--up` then starts the stack there. |
| `misty cloud prod check\|up\|down\|logs [service]\|enable-writer --yes` | Operates Misty Cloud's production stack (the server and billing); run it on the VPS, in the deploy folder (`~/misty-cloud`). |
| `misty build` | Builds the app, step by step. |
| `misty release [--dry-run]` | Publishes a release, step by step: start it, build it on each system, make it public. |

## Settings

Settings live outside every repository, in files only you can read:

- `.env/server/` in the data folder `~/.misty/misty.conf` names (`misty.root`, `~/.misty/dev/default` when there is none) holds the server's settings and development keys (`misty setup server`). `MISTY_ENV_DIR` names another folder.
- `~/.misty/misty.conf` can also give non-secret environment defaults in `[server]` and `[cli]`, used only where the shell and the `.env` files leave a name unset. Secrets never go there.
- `misty-cli/.env/` holds `common.env`, `cloudflare.env` and `release.env` (git-ignored).

`misty env describe` lists every server setting and the file it belongs in. Values are never printed.

## Release keys

The updater signing key lives in `~/.config/misty-release/`. Create it once, or check a restored
one against misty's `release/trust.json`:

```sh
node tasks/release-keys.ts
```

## License

[AGPL-3.0-or-later](LICENSE), with the attribution term in [NOTICE](NOTICE).
