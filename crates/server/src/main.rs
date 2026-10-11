//! `myledger-server` — the MyLedger backend binary.
//!
//! Two subcommands: `serve` (the default) and `setup`. Argument parsing is by
//! hand because there is very little of it, and a dependency that exists only
//! to read two flags earns its keep in compile time rather than clarity.

use std::process::ExitCode;

use myledger_core::APP_NAME;
use myledger_db::auth as db_auth;
use myledger_server::config::Config;
use myledger_server::{AppState, build_app, create_user, open_database};
use tracing_subscriber::EnvFilter;

/// Environment variables the first-run setup reads.
const SETUP_USERNAME_ENV: &str = "MYLEDGER_SETUP_USERNAME";
const SETUP_PASSWORD_ENV: &str = "MYLEDGER_SETUP_PASSWORD";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("MYLEDGER_LOG")
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("\n{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let command = Command::parse(std::env::args().skip(1))?;
    let config = Config::from_env()?;
    let db = open_database(&config.db_path).await?;

    match command {
        Command::Help => {
            print_usage();
            Ok(())
        }
        Command::Setup { username, password } => {
            let username = username
                .or_else(|| std::env::var(SETUP_USERNAME_ENV).ok())
                .ok_or_else(|| {
                    format!("setup needs a username: pass --username or set {SETUP_USERNAME_ENV}")
                })?;
            let password = password
                .or_else(|| std::env::var(SETUP_PASSWORD_ENV).ok())
                .ok_or_else(|| {
                    format!("setup needs a password: pass --password or set {SETUP_PASSWORD_ENV}")
                })?;

            let created = create_user(&db, &username, &password).await?;
            println!("Created the {APP_NAME} user {created:?}. You can now start the server.");
            Ok(())
        }
        Command::Serve => serve(db, config).await,
    }
}

async fn serve(db: myledger_db::Db, config: Config) -> Result<(), String> {
    if db_auth::count_users(&db).await.map_err(|e| e.to_string())? == 0 {
        // Setup from the environment if it was provided, so that a container
        // can come up configured on its first boot without a second command.
        match (std::env::var(SETUP_USERNAME_ENV), std::env::var(SETUP_PASSWORD_ENV)) {
            (Ok(username), Ok(password)) => {
                let created = create_user(&db, &username, &password).await?;
                tracing::info!(username = %created, "completed first-run setup from the environment");
            }
            _ => return Err(first_run_instructions()),
        }
    }

    if !config.cookie_secure {
        tracing::warn!(
            "session cookies are not marked Secure; set MYLEDGER_COOKIE_SECURE=true when serving over HTTPS"
        );
    }

    let bind = config.bind;
    let state = AppState::new(db, config);
    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| format!("could not listen on {bind}: {e}"))?;

    tracing::info!("{APP_NAME} listening on http://{bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| format!("the server stopped unexpectedly: {e}"))
}

/// Waits for Ctrl-C so in-flight requests finish before the process exits.
async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_ok() {
        tracing::info!("shutting down");
    }
}

fn first_run_instructions() -> String {
    format!(
        "\
{APP_NAME} has no user yet, so there is nobody to sign in as. This is a
one-time setup step. Do either of the following, then start the server again.

  1. Run the setup subcommand:

       myledger-server setup --username <name> --password <password>

  2. Or set both environment variables and start the server; it will
     complete setup on the spot:

       {SETUP_USERNAME_ENV}=<name>
       {SETUP_PASSWORD_ENV}=<password>

The password is hashed with argon2id and is never stored in the clear."
    )
}

fn print_usage() {
    println!(
        "\
{APP_NAME} server

USAGE:
    myledger-server [serve]     Run the API server (the default).
    myledger-server setup       Create the single user, once.
    myledger-server --help      Show this message.

SETUP OPTIONS:
    --username <name>           Otherwise read from {SETUP_USERNAME_ENV}.
    --password <password>       Otherwise read from {SETUP_PASSWORD_ENV}.

ENVIRONMENT:
    MYLEDGER_DB_PATH            SQLite file          (default data/myledger.sqlite)
    MYLEDGER_DATA_DIR           Uploaded documents   (default data/documents)
    MYLEDGER_BIND               Listen address       (default 127.0.0.1:8787)
    MYLEDGER_COOKIE_SECURE      Mark session cookies Secure; turn on for HTTPS.
    MYLEDGER_LOG                Log filter           (default info)"
    )
}

/// What the user asked for.
enum Command {
    Serve,
    Setup { username: Option<String>, password: Option<String> },
    Help,
}

impl Command {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let args: Vec<String> = args.collect();
        match args.first().map(String::as_str) {
            None | Some("serve") => Ok(Self::Serve),
            Some("-h" | "--help" | "help") => Ok(Self::Help),
            Some("setup") => Self::parse_setup(&args[1..]),
            Some(other) => {
                Err(format!("unknown command {other:?}. Try `myledger-server --help`."))
            }
        }
    }

    fn parse_setup(args: &[String]) -> Result<Self, String> {
        let (mut username, mut password) = (None, None);
        let mut rest = args.iter();

        while let Some(flag) = rest.next() {
            let target = match flag.as_str() {
                "--username" | "-u" => &mut username,
                "--password" | "-p" => &mut password,
                other => {
                    return Err(format!(
                        "unknown option {other:?} for `setup`. Try `myledger-server --help`."
                    ));
                }
            };
            let value = rest
                .next()
                .ok_or_else(|| format!("{flag} needs a value"))?;
            *target = Some(value.clone());
        }

        Ok(Self::Setup { username, password })
    }
}
