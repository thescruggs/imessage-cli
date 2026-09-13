use anyhow::{anyhow, Result};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub server: String,
    pub token: String,
}

fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/imsg")
}

pub fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".cache/imsg")
}

fn read_kv(path: &PathBuf, key: &str) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    for line in s.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim();
            if let Some(v) = rest.strip_prefix('=') {
                return Some(v.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

/// Resolve server + token from CLI flags, then ~/.config/imsg/client.toml,
/// then (same machine) ~/.config/imsg/server.toml with localhost.
pub fn load(server: Option<String>, token: Option<String>) -> Result<Config> {
    let dir = config_dir();
    let client = dir.join("client.toml");
    let srv = server
        .or_else(|| read_kv(&client, "server"))
        .unwrap_or_else(|| "http://127.0.0.1:8787".to_string());
    let tok = token
        .or_else(|| read_kv(&client, "token"))
        .or_else(|| read_kv(&dir.join("server.toml"), "token"))
        .ok_or_else(|| anyhow!("no token: pass --token, or create {} with server = \"http://host:8787\" and token = \"...\"", client.display()))?;
    Ok(Config { server: srv.trim_end_matches('/').to_string(), token: tok })
}

pub fn save(cfg: &Config) -> Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("client.toml"), format!("server = \"{}\"\ntoken = \"{}\"\n", cfg.server, cfg.token))?;
    Ok(())
}
