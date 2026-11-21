//! Runs a parsed command after loading the environment files it needs.

use super::*;

pub fn dispatch(arguments: Cli, settings: Settings) -> Result<()> {
    let Some(command) = arguments.command else {
        crate::config::load_cli_environment(&settings.workspace, GUIDED_ENVIRONMENT)?;
        return crate::guide::menu(&settings);
    };
    load_command_environment(&command, &settings)?;
    if matches!(command, Command::Server(_)) {
        settings.workspace.require_server()?;
    }
    match command {
        Command::Setup(options) => crate::setup::run(&settings.workspace, options),
        Command::Doctor(options) => {
            if options.target == crate::diagnostics::Target::Release {
                if options.json || options.fix {
                    anyhow::bail!(
                        "release doctor supports text diagnostics only; omit --json and --fix"
                    );
                }
                doctor(&settings)
            } else {
                crate::diagnostics::run(&settings.workspace, options)
            }
        }
        Command::Env(command) => match command.command {
            EnvCommand::Describe => environment::describe(),
            EnvCommand::Set { name } => {
                use std::io::Read;
                let mut value = String::new();
                std::io::stdin().read_to_string(&mut value)?;
                environment::set(
                    &settings.workspace,
                    &name,
                    value.trim_end_matches(['\r', '\n']),
                )
            }
            EnvCommand::Init => {
                environment::init(&settings.workspace)?;
                server::initialize_development_secrets(&settings.workspace)
            }
            EnvCommand::Check => environment::check(&settings.workspace),
            EnvCommand::Status => environment::status(&settings.workspace),
        },
        Command::Check(command) => match command.target {
            CheckTarget::App => checks::app(&settings.workspace),
            CheckTarget::Scripts => crate::process::CommandSpec::new(crate::process::npm())
                .args(["run", "test:scripts"])
                .run(&settings.workspace.misty),
            CheckTarget::Server => checks::server(&settings.workspace),
            CheckTarget::GoSizes => {
                settings.workspace.require_server()?;
                checks::go_file_sizes(&settings.workspace.server)
            }
            CheckTarget::Website => checks::website(&settings.workspace),
            CheckTarget::Docs => checks::docs(&settings.workspace),
            CheckTarget::Features => checks::features(&settings.workspace),
            CheckTarget::Acceptance => checks::acceptance(&settings.workspace),
            CheckTarget::Cli => checks::cli(&settings.workspace),
            CheckTarget::All => checks::all(&settings.workspace),
        },
        Command::Hook(hook) => crate::hooks::dispatch(&settings.workspace, hook.command),
        Command::Cloud(cloud) => crate::cloud::dispatch(&settings.workspace, cloud.command),
        Command::Build => crate::guide::build(&settings.workspace),
        Command::Desktop(command) => match command.command {
            DesktopCommand::Dev { route } => desktop::dev(&settings.workspace, route.as_deref()),
            DesktopCommand::Clean { apply } => desktop::clean(&settings.workspace, apply),
            DesktopCommand::Icons { command } => match command {
                IconCommand::Sync { source } => {
                    desktop::sync_icons(&settings.workspace, source.as_deref())
                }
            },
        },

        Command::Website(command) => match command.command {
            WebsiteCommand::Dev => website::dev(&settings.workspace),
        },
        Command::Server(command) => match command.command {
            ServerCommand::Up {
                no_build,
                verbose,
                gui,
            } => {
                server::up(&settings.workspace, !no_build, verbose)?;
                if gui {
                    crate::console::open(&settings.workspace)
                } else {
                    println!("Console: misty server up --gui");
                    Ok(())
                }
            }
            ServerCommand::Status => server::status(&settings.workspace),
            ServerCommand::Deploy => server::deploy_development(&settings.workspace),
            ServerCommand::Url => server::url(&settings.workspace),
            ServerCommand::Down { volumes } => {
                crate::console::stop(&settings.workspace)?;
                server::down(&settings.workspace, volumes)
            }
            ServerCommand::Logs {
                service,
                follow,
                tail,
            } => server::logs(&settings.workspace, service.as_deref(), follow, tail),
            ServerCommand::Test {
                isolated: true,
                arguments,
            } => crate::hooks::run_isolated_server_tests(&settings.workspace.server, &arguments),
            ServerCommand::Test { arguments, .. } => {
                server::test(&settings.workspace.server, &arguments)
            }
            ServerCommand::Goose { arguments } => {
                server::goose(&settings.workspace.server, &arguments)
            }
            ServerCommand::StackTest => server::stack_test(&settings.workspace.server),
            ServerCommand::Worker { command } => match command {
                WorkerCommand::GenerateSecrets => {
                    server::generate_worker_secrets(&settings.workspace)
                }
            },
            ServerCommand::R2 { command } => match command {
                R2Command::ConfigureCors { apply } => {
                    server::configure_r2_cors(&settings.workspace, apply)
                }
            },
        },
        Command::Release(command) => crate::guide::release(&settings.workspace, command.dry_run),
    }
}

/// The guided flows reach builds and releases, so they read what those need.
const GUIDED_ENVIRONMENT: &[&str] = &["common.env", "release.env"];

pub(super) fn load_command_environment(command: &Command, settings: &Settings) -> Result<()> {
    let files: &[&str] = match command {
        // Hooks and production run on any checkout with nothing configured.
        Command::Env(_) | Command::Hook(_) | Command::Cloud(_) => &[],
        Command::Setup(options)
            if matches!(options.component, crate::setup::Component::Cloudflare) =>
        {
            &["common.env", "cloudflare.env"]
        }
        Command::Setup(_) => &["common.env"],
        Command::Doctor(options) if options.target == crate::diagnostics::Target::Release => {
            &["common.env", "release.env"]
        }
        Command::Doctor(options) if options.target == crate::diagnostics::Target::Cloudflare => {
            &["common.env", "cloudflare.env"]
        }
        Command::Doctor(_) => &["common.env"],
        Command::Release(_) | Command::Build => GUIDED_ENVIRONMENT,
        Command::Desktop(_) => &["common.env"],
        Command::Server(server) => match &server.command {
            ServerCommand::Worker { .. } | ServerCommand::R2 { .. } => {
                &["common.env", "cloudflare.env"]
            }
            _ => &["common.env"],
        },
        Command::Check(_) | Command::Website(_) => &["common.env"],
    };
    crate::config::load_cli_environment(&settings.workspace, files)?;
    // Tests and goose choose their own database settings; the stack's must not leak in.
    if let Command::Server(server) = command {
        if !matches!(
            server.command,
            ServerCommand::Test { .. } | ServerCommand::Goose { .. } | ServerCommand::StackTest
        ) {
            environment::apply(&settings.workspace)?;
            // misty.conf's [server] defaults, for whatever the .env files and the shell left unset.
            crate::misty_conf::apply_defaults("server");
        }
    }
    Ok(())
}
