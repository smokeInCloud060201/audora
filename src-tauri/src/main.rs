//! Audora binary entrypoint.

use std::env;
use std::path::PathBuf;

use audora_core::build_production_dispatcher;

fn default_database_path() -> PathBuf {
    if let Ok(custom) = env::var("AUDORA_DB_PATH") {
        return PathBuf::from(custom);
    }
    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_owned());

    if cfg!(target_os = "macos") {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("com.audora.app")
            .join("audora.sqlite")
    } else if cfg!(target_os = "windows") {
        let appdata = env::var("APPDATA").unwrap_or(home);
        PathBuf::from(appdata)
            .join("com.audora.app")
            .join("data")
            .join("audora.sqlite")
    } else {
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("audora")
            .join("audora.sqlite")
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let db_path = default_database_path();
    match build_production_dispatcher(&db_path) {
        Ok(dispatcher) => {
            let settings = dispatcher.get_settings().unwrap_or_default();
            tracing::info!(
                mode = ?settings.window_mode,
                theme = ?settings.theme,
                "Audora application core initialized"
            );
        }
        Err(err) => {
            tracing::error!(
                code = err.to_payload().code.as_str(),
                "Audora failed to initialize persistence core"
            );
            std::process::exit(1);
        }
    }
}
