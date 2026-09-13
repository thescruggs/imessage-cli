use crate::config::Config;
use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use imsg_core::*;
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct Api {
    cfg: Config,
    http: reqwest::Client,
}

impl Api {
    pub fn new(cfg: Config) -> Self {
        Api { cfg, http: reqwest::Client::builder().timeout(std::time::Duration::from_secs(120)).build().unwrap() }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.cfg.server, path)
    }

    fn ws_url(&self) -> String {
        let base = self.cfg.server.replacen("http", "ws", 1);
        format!("{base}/ws?token={}", self.cfg.token)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let r = self.http.get(self.url(path)).bearer_auth(&self.cfg.token).send().await?;
        if !r.status().is_success() {
            return Err(anyhow!("{} -> {}", path, r.status()));
        }
        Ok(r.json().await?)
    }

    pub async fn info(&self) -> Result<ServerInfo> {
        self.get("/api/info").await
    }
    pub async fn chats(&self) -> Result<Vec<Chat>> {
        self.get("/api/chats?limit=300").await
    }
    pub async fn contacts(&self) -> Result<Vec<Contact>> {
        self.get("/api/contacts").await
    }
    pub async fn messages(&self, chat_id: i64, before: Option<i64>, limit: i64) -> Result<MessagesPage> {
        let q = match before {
            Some(b) => format!("/api/chats/{chat_id}/messages?limit={limit}&before={b}"),
            None => format!("/api/chats/{chat_id}/messages?limit={limit}"),
        };
        self.get(&q).await
    }
    pub async fn search(&self, q: &str) -> Result<Vec<Message>> {
        self.get(&format!("/api/search?limit=100&q={}", urlencode(q))).await
    }
    pub async fn send(&self, req: &SendRequest) -> Result<SendResponse> {
        let r = self.http.post(self.url("/api/send")).bearer_auth(&self.cfg.token).json(req).send().await?;
        Ok(r.json().await?)
    }
    pub async fn upload(&self, path: &PathBuf) -> Result<UploadResponse> {
        let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let bytes = tokio::fs::read(path).await?;
        let part = reqwest::multipart::Part::bytes(bytes).file_name(name);
        let form = reqwest::multipart::Form::new().part("file", part);
        let r = self.http.post(self.url("/api/upload")).bearer_auth(&self.cfg.token).multipart(form).send().await?;
        if !r.status().is_success() {
            return Err(anyhow!("upload failed: {}", r.status()));
        }
        Ok(r.json().await?)
    }
    /// Download an attachment into the local cache and return its path.
    pub async fn download(&self, att: &Attachment) -> Result<PathBuf> {
        let dir = crate::config::cache_dir();
        tokio::fs::create_dir_all(&dir).await?;
        let safe: String = att.name.chars().filter(|c| !matches!(c, '/' | '\\')).collect();
        let dest = dir.join(format!("{}-{}", att.id, safe));
        if dest.exists() {
            return Ok(dest);
        }
        let r = self.http.get(self.url(&att.url)).bearer_auth(&self.cfg.token).send().await?;
        if !r.status().is_success() {
            return Err(anyhow!("download failed: {}", r.status()));
        }
        let bytes = r.bytes().await?;
        tokio::fs::write(&dest, &bytes).await?;
        Ok(dest)
    }

    /// Run the websocket forever, forwarding events. Reconnects with backoff.
    pub async fn ws_loop(&self, tx: mpsc::UnboundedSender<WsEvent>) {
        let mut delay = 1;
        loop {
            match tokio_tungstenite::connect_async(self.ws_url()).await {
                Ok((ws, _)) => {
                    delay = 1;
                    let (_, mut read) = ws.split();
                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(tokio_tungstenite::tungstenite::Message::Text(t)) => {
                                if let Ok(ev) = serde_json::from_str::<WsEvent>(&t) {
                                    if tx.send(ev).is_err() {
                                        return;
                                    }
                                }
                            }
                            Ok(tokio_tungstenite::tungstenite::Message::Close(_)) | Err(_) => break,
                            _ => {}
                        }
                    }
                    let _ = tx.send(WsEvent::Error { message: "disconnected, reconnecting…".into() });
                }
                Err(e) => {
                    let _ = tx.send(WsEvent::Error { message: format!("ws: {e}") });
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            delay = (delay * 2).min(30);
        }
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
