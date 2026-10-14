mod api;
mod auth;
mod db;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::DefaultBodyLimit;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use clap::{Parser, Subcommand};

use db::Db;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub data_dir: PathBuf,
    pub secure_cookies: bool,
    pub failures: Arc<Mutex<HashMap<String, (u32, i64)>>>,
    pub dummy_hash: Arc<String>,
}

#[derive(Parser)]
#[command(name = "whidictate-server", version, about = "whidictate Server")]
struct Cli {
    /// Datenverzeichnis (Datenbank und Audiodateien)
    #[arg(long, env = "WHIDICTATE_DATA", default_value = "./data", global = true)]
    data_dir: PathBuf,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Server starten (Standard)
    Serve {
        #[arg(long, env = "WHIDICTATE_LISTEN", default_value = "127.0.0.1:8484")]
        listen: String,
        /// Cookies nur über HTTPS senden (setzen, wenn ein TLS-Proxy davor steht)
        #[arg(long, env = "WHIDICTATE_SECURE_COOKIES")]
        secure_cookies: bool,
    },
    /// Benutzer anlegen. Passwort über WHIDICTATE_PASSWORD oder interaktiv.
    CreateUser {
        username: String,
        #[arg(long)]
        display_name: Option<String>,
        /// author, secretary oder admin
        #[arg(long, default_value = "author")]
        role: String,
    },
}

pub fn build_state(data_dir: PathBuf, secure_cookies: bool) -> anyhow::Result<AppState> {
    std::fs::create_dir_all(data_dir.join("audio"))?;
    let db = Db::open(&data_dir.join("whidictate.db"))?;
    Ok(AppState {
        db: Arc::new(db),
        data_dir,
        secure_cookies,
        failures: Arc::default(),
        dummy_hash: Arc::new(auth::hash_password("dummy-password")?),
    })
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(|| async { Html(include_str!("../ui/index.html")) }))
        .merge(api::router())
        .layer(DefaultBodyLimit::max(300 * 1024 * 1024))
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let cli = Cli::parse();
    let cmd = cli.cmd.unwrap_or(Cmd::Serve { listen: "127.0.0.1:8484".into(), secure_cookies: false });
    match cmd {
        Cmd::CreateUser { username, display_name, role } => {
            let st = build_state(cli.data_dir, false)?;
            let password = match std::env::var("WHIDICTATE_PASSWORD") {
                Ok(p) => p,
                Err(_) => {
                    eprintln!("Passwort für {username} (min. 8 Zeichen): ");
                    let mut s = String::new();
                    std::io::stdin().read_line(&mut s)?;
                    s.trim_end().to_string()
                }
            };
            let u = api::NewUser { display_name: display_name.unwrap_or_else(|| username.clone()), username, password, role };
            api::insert_user(&st, &u).map_err(|e| anyhow::anyhow!(e))?;
            println!("Benutzer {} angelegt.", u.username);
        }
        Cmd::Serve { listen, secure_cookies } => {
            let st = build_state(cli.data_dir, secure_cookies)?;
            // Erststart per Umgebungsvariable: ersten Administrator anlegen.
            if st.db.user_count() == 0 {
                if let Ok(pw) = std::env::var("WHIDICTATE_ADMIN_PASSWORD") {
                    let u = api::NewUser { username: "admin".into(), display_name: "Administrator".into(), password: pw, role: "admin".into() };
                    api::insert_user(&st, &u).map_err(|e| anyhow::anyhow!(e))?;
                    tracing::info!("Administrator 'admin' angelegt");
                } else {
                    tracing::warn!("Noch kein Benutzer vorhanden. Anlegen mit: whidictate-server create-user <name> --role admin");
                }
            }
            let listener = tokio::net::TcpListener::bind(&listen).await?;
            tracing::info!("whidictate-server läuft auf http://{listen}");
            axum::serve(listener, app(st))
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
        }
    }
    Ok(())
}
