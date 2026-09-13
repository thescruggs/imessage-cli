mod contacts;
mod db;
mod send;

use anyhow::{anyhow, Context, Result};
use axum::{
    body::Body,
    extract::{
        ws::{Message as WsMsg, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Multipart, Path as AxPath, Query, Request, State,
    },
    http::{header, HeaderMap, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use contacts::ContactBook;
use db::Db;
use futures_util::{SinkExt, StreamExt};
use imsg_core::*;
use serde::Deserialize;
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, SystemTime},
};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(name = "imsg-server", version, about = "Serves your Mac's iMessage history to imsg clients")]
struct Args {
    /// Address to bind (defaults to all IPv4 interfaces; use 127.0.0.1:8787 for local-only access).
    #[arg(long, env = "IMSG_BIND", default_value = "0.0.0.0:8787")]
    bind: SocketAddr,
    /// Auth token. Generated and stored in ~/.config/imsg/server.toml if absent.
    #[arg(long, env = "IMSG_TOKEN")]
    token: Option<String>,
    /// Path to chat.db (defaults to ~/Library/Messages/chat.db).
    #[arg(long, env = "IMSG_DB")]
    db: Option<PathBuf>,
    /// Serve web client from this directory instead of the embedded copy (dev mode).
    #[arg(long, env = "IMSG_WEB_DIR")]
    web_dir: Option<PathBuf>,
    /// Poll interval for new messages, in milliseconds.
    #[arg(long, default_value_t = 1000)]
    poll_ms: u64,
}

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Db>>,
    contacts: Arc<RwLock<ContactBook>>,
    token: Arc<String>,
    tx: broadcast::Sender<WsEvent>,
    cache_dir: PathBuf,
    upload_dir: PathBuf,
    info: Arc<ServerInfo>,
    web_dir: Option<PathBuf>,
}

fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/imsg")
}

fn load_or_create_token(cli: Option<String>) -> Result<String> {
    if let Some(t) = cli {
        return Ok(t);
    }
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    let f = dir.join("server.toml");
    if let Ok(s) = std::fs::read_to_string(&f) {
        for line in s.lines() {
            if let Some(v) = line.trim().strip_prefix("token") {
                let v = v.trim().trim_start_matches('=').trim().trim_matches('"');
                if !v.is_empty() {
                    return Ok(v.to_string());
                }
            }
        }
    }
    use rand::Rng;
    let token: String = rand::rng().sample_iter(rand::distr::Alphanumeric).take(32).map(char::from).collect();
    std::fs::write(&f, format!("# imsg server config\ntoken = \"{token}\"\n"))?;
    info!("generated new token, saved to {}", f.display());
    Ok(token)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=warn".into()))
        .init();
    let args = Args::parse();
    let token = load_or_create_token(args.token)?;
    let db_path = args.db.unwrap_or_else(|| db::expand_home("~/Library/Messages/chat.db"));
    let db = Db::open(&db_path).with_context(|| {
        format!(
            "cannot open {}. Grant Full Disk Access to imsg-server (System Settings > Privacy & Security > Full Disk Access); when running it by hand from a terminal, the terminal app needs it too.",
            db_path.display()
        )
    })?;
    let me = db.my_addresses().unwrap_or_default();
    let book = contacts::load().unwrap_or_else(|e| {
        warn!("contacts unavailable: {e}");
        ContactBook::default()
    });
    info!("contacts: {} entries", book.contacts.len());
    let cache_dir = config_dir().join("cache");
    // Messages.app is sandboxed: files sent via AppleScript from hidden or
    // app-private directories (~/.config, ~/Library/Application Support) are
    // accepted but silently fail with error 25. ~/Pictures is readable, so
    // staged uploads live there.
    let upload_dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join("Pictures/imsg-uploads");
    std::fs::create_dir_all(&cache_dir)?;
    std::fs::create_dir_all(&upload_dir)?;
    let (tx, _) = broadcast::channel(256);
    let host = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        contacts: Arc::new(RwLock::new(book)),
        token: Arc::new(token.clone()),
        tx,
        cache_dir,
        upload_dir,
        info: Arc::new(ServerInfo { name: "imsg-server".into(), version: env!("CARGO_PKG_VERSION").into(), me, host }),
        web_dir: args.web_dir,
    };

    tokio::spawn(watcher(state.clone(), Duration::from_millis(args.poll_ms), db_path.clone()));
    tokio::spawn(contacts_refresher(state.clone()));

    let api = Router::new()
        .route("/info", get(info_handler))
        .route("/chats", get(chats_handler))
        .route("/chats/{id}", get(chat_handler))
        .route("/chats/{id}/messages", get(messages_handler))
        .route("/messages/{guid}", get(message_handler))
        .route("/contacts", get(contacts_handler))
        .route("/contacts/{id}/photo", get(contact_photo_handler))
        .route("/attachments/{id}", get(attachment_handler))
        .route("/search", get(search_handler))
        .route("/upload", post(upload_handler))
        .route("/send", post(send_handler))
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));

    let app = Router::new()
        .nest("/api", api)
        .route("/ws", get(ws_handler))
        .route("/", get(static_handler))
        .route("/{*path}", get(static_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    info!("imsg-server listening on http://{}  (token: {})", args.bind, token);
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------- auth ----------

fn token_from(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    if let Some(v) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(t) = v.strip_prefix("Bearer ") {
            return Some(t.trim().to_string());
        }
    }
    if let Some(q) = uri.query() {
        for pair in q.split('&') {
            if let Some(v) = pair.strip_prefix("token=") {
                return Some(v.to_string());
            }
        }
    }
    if let Some(c) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        for part in c.split(';') {
            if let Some(v) = part.trim().strip_prefix("imsg_token=") {
                return Some(v.to_string());
            }
        }
    }
    None
}

async fn auth(State(st): State<AppState>, req: Request, next: Next) -> Response {
    match token_from(req.headers(), req.uri()) {
        Some(t) if t == *st.token => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "unauthorized"}))).into_response(),
    }
}

// ---------- helpers ----------

struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        error!("{:#}", self.0);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("{:#}", self.0)}))).into_response()
    }
}
type ApiResult<T> = std::result::Result<T, ApiError>;

async fn with_db<T: Send + 'static>(st: &AppState, f: impl FnOnce(&mut Db, &ContactBook) -> Result<T> + Send + 'static) -> Result<T> {
    let db = st.db.clone();
    let contacts = st.contacts.clone();
    tokio::task::spawn_blocking(move || {
        let book = contacts.read().map_err(|_| anyhow!("contacts lock"))?.clone();
        let mut db = db.lock().map_err(|_| anyhow!("db lock"))?;
        f(&mut db, &book)
    })
    .await?
}

// ---------- handlers ----------

async fn info_handler(State(st): State<AppState>) -> Json<ServerInfo> {
    Json((*st.info).clone())
}

#[derive(Deserialize)]
struct ChatsQuery {
    limit: Option<i64>,
}

async fn chats_handler(State(st): State<AppState>, Query(q): Query<ChatsQuery>) -> ApiResult<Json<Vec<Chat>>> {
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    Ok(Json(with_db(&st, move |db, book| db.chats(limit, book)).await?))
}

async fn chat_handler(State(st): State<AppState>, AxPath(id): AxPath<i64>) -> ApiResult<Response> {
    match with_db(&st, move |db, book| db.chat_by_id(id, book)).await? {
        Some(c) => Ok(Json(c).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

#[derive(Deserialize)]
struct MsgQuery {
    before: Option<i64>,
    limit: Option<i64>,
}

async fn messages_handler(State(st): State<AppState>, AxPath(id): AxPath<i64>, Query(q): Query<MsgQuery>) -> ApiResult<Json<MessagesPage>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    Ok(Json(with_db(&st, move |db, book| db.messages(id, q.before, limit, book)).await?))
}

async fn message_handler(State(st): State<AppState>, AxPath(guid): AxPath<String>) -> ApiResult<Response> {
    match with_db(&st, move |db, book| db.message_by_guid(&guid, book)).await? {
        Some(m) => Ok(Json(m).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

async fn contacts_handler(State(st): State<AppState>) -> ApiResult<Json<Vec<Contact>>> {
    let book = st.contacts.read().map_err(|_| anyhow!("lock"))?;
    Ok(Json(book.contacts.clone()))
}

async fn contact_photo_handler(State(st): State<AppState>, AxPath(id): AxPath<i64>) -> ApiResult<Response> {
    let book = st.contacts.read().map_err(|_| anyhow!("lock"))?;
    match book.photo(id) {
        Some(bytes) => {
            let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "image/png" } else { "image/jpeg" };
            Ok(([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "private, max-age=86400")], bytes.clone()).into_response())
        }
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

#[derive(Deserialize)]
struct AttQuery {
    /// "1" to get a browser-friendly preview (HEIC -> JPEG, downscaled).
    preview: Option<String>,
    download: Option<String>,
}

async fn attachment_handler(State(st): State<AppState>, AxPath(id): AxPath<i64>, Query(q): Query<AttQuery>, req: Request) -> ApiResult<Response> {
    let Some((path, mime, name)) = with_db(&st, move |db, _| db.attachment_path(id)).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if !path.exists() {
        return Ok((StatusCode::NOT_FOUND, "attachment not downloaded on this Mac").into_response());
    }
    let (path, mime) = if q.preview.is_some() && mime.starts_with("image/") && mime != "image/gif" {
        match preview_image(&st.cache_dir, &path, id).await {
            Ok(p) => (p, "image/jpeg".to_string()),
            Err(e) => {
                warn!("preview failed for {}: {e}", path.display());
                (path, mime)
            }
        }
    } else if mime == "image/heic" || mime == "image/heif" {
        // Browsers other than Safari cannot display HEIC; convert full-size.
        match convert_heic(&st.cache_dir, &path, id, None).await {
            Ok(p) => (p, "image/jpeg".to_string()),
            Err(_) => (path, mime),
        }
    } else {
        (path, mime)
    };
    let mut svc = tower_http::services::ServeFile::new_with_mime(&path, &mime.parse().unwrap_or(mime_guess::mime::APPLICATION_OCTET_STREAM));
    let mut resp = tower::ServiceExt::oneshot(&mut svc, req).await.map_err(|e| anyhow!("serve: {e}"))?.into_response();
    let disp = if q.download.is_some() { "attachment" } else { "inline" };
    let safe = name.replace('"', "");
    if let Ok(v) = format!("{disp}; filename=\"{safe}\"").parse() {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    resp.headers_mut().insert(header::CACHE_CONTROL, "private, max-age=86400".parse().unwrap());
    Ok(resp)
}

async fn convert_heic(cache: &PathBuf, src: &PathBuf, id: i64, max: Option<u32>) -> Result<PathBuf> {
    let out = cache.join(format!("att-{id}-{}.jpg", max.map(|m| m.to_string()).unwrap_or_else(|| "full".into())));
    if out.exists() {
        return Ok(out);
    }
    let mut cmd = tokio::process::Command::new("sips");
    cmd.args(["-s", "format", "jpeg", "-s", "formatOptions", "85"]);
    if let Some(m) = max {
        cmd.args(["-Z", &m.to_string()]);
    }
    let o = cmd.arg(src).arg("--out").arg(&out).output().await?;
    if !o.status.success() {
        return Err(anyhow!("sips: {}", String::from_utf8_lossy(&o.stderr)));
    }
    Ok(out)
}

async fn preview_image(cache: &PathBuf, src: &PathBuf, id: i64) -> Result<PathBuf> {
    convert_heic(cache, src, id, Some(1400)).await
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    limit: Option<i64>,
}

async fn search_handler(State(st): State<AppState>, Query(q): Query<SearchQuery>) -> ApiResult<Json<Vec<Message>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let term = q.q.clone();
    Ok(Json(with_db(&st, move |db, book| db.search(&term, limit, book)).await?))
}

async fn upload_handler(State(st): State<AppState>, mut mp: Multipart) -> ApiResult<Json<UploadResponse>> {
    while let Some(field) = mp.next_field().await? {
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().map(sanitize_name).unwrap_or_else(|| "upload".into());
        let id = format!("{:x}", SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_nanos());
        let dir = st.upload_dir.join(&id);
        tokio::fs::create_dir_all(&dir).await?;
        let dest = dir.join(&name);
        let bytes = field.bytes().await?;
        tokio::fs::write(&dest, &bytes).await?;
        return Ok(Json(UploadResponse { id, name, size: bytes.len() as u64 }));
    }
    Err(anyhow!("no file field").into())
}

fn sanitize_name(s: &str) -> String {
    let s: String = s.chars().filter(|c| !matches!(c, '/' | '\\' | '\0')).collect();
    if s.is_empty() { "upload".into() } else { s }
}

async fn send_handler(State(st): State<AppState>, Json(req): Json<SendRequest>) -> ApiResult<Json<SendResponse>> {
    // Resolve destination: chat guid, or a single address (participant), or existing chat for addresses.
    let mut chat_guid = req.chat_guid.clone().filter(|s| !s.is_empty());
    let mut address: Option<String> = None;
    if chat_guid.is_none() {
        if req.to.is_empty() {
            return Ok(Json(SendResponse { ok: false, detail: Some("chat_guid or to[] required".into()) }));
        }
        let to = req.to.clone();
        if let Some(c) = with_db(&st, move |db, book| db.chat_for_addresses(&to, book)).await? {
            chat_guid = Some(c.guid);
        } else if req.to.len() == 1 {
            address = Some(req.to[0].clone());
        } else {
            return Ok(Json(SendResponse { ok: false, detail: Some("new group chats must be started from Messages.app first".into()) }));
        }
    }
    // Collect files.
    let mut files: Vec<PathBuf> = vec![];
    for id in &req.uploads {
        let dir = st.upload_dir.join(sanitize_name(id));
        if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
            if let Ok(Some(e)) = rd.next_entry().await {
                files.push(e.path());
            }
        }
    }
    for p in &req.paths {
        let p = db::expand_home(p);
        if p.exists() {
            files.push(p);
        } else {
            return Ok(Json(SendResponse { ok: false, detail: Some(format!("no such file: {}", p.display())) }));
        }
    }
    let text = req.text.clone().map(|t| t.trim_end_matches('\n').to_string()).filter(|t| !t.is_empty());
    if text.is_none() && files.is_empty() {
        return Ok(Json(SendResponse { ok: false, detail: Some("nothing to send".into()) }));
    }
    // Messages.app only exposes recent chats to AppleScript. If a 1:1 chat id is
    // not resolvable, fall back to addressing the participant directly.
    let fallback_addr: Option<String> = match (&chat_guid, &address) {
        (Some(g), None) if g.contains(";-;") => g.rsplit(';').next().map(str::to_string),
        _ => None,
    };
    let r: Result<()> = async {
        let send_one = |what: String, is_file: bool| {
            let chat_guid = chat_guid.clone();
            let address = address.clone();
            let fallback_addr = fallback_addr.clone();
            async move {
                let first: Result<()> = match (&chat_guid, &address) {
                    (Some(g), _) => if is_file { send::send_file_to_chat(g, &what).await } else { send::send_text_to_chat(g, &what).await },
                    (None, Some(a)) => if is_file { send::send_file_to_address(a, &what).await } else { send::send_text_to_address(a, &what).await },
                    _ => unreachable!(),
                };
                match (first, fallback_addr) {
                    (Err(e), Some(a)) if e.to_string().contains("Can’t get chat id") || e.to_string().contains("Can't get chat id") => {
                        warn!("chat id not scriptable, retrying via participant {a}");
                        if is_file { send::send_file_to_address(&a, &what).await } else { send::send_text_to_address(&a, &what).await }
                    }
                    (r, _) => r,
                }
            }
        };
        for f in &files {
            send_one(f.to_string_lossy().to_string(), true).await?;
        }
        if let Some(t) = &text {
            send_one(t.clone(), false).await?;
        }
        Ok(())
    }
    .await;
    match r {
        Ok(()) => Ok(Json(SendResponse { ok: true, detail: None })),
        Err(e) => Ok(Json(SendResponse { ok: false, detail: Some(format!("{e:#}")) })),
    }
}

// ---------- websocket ----------

async fn ws_handler(State(st): State<AppState>, ws: WebSocketUpgrade, headers: HeaderMap, uri: Uri) -> Response {
    match token_from(&headers, &uri) {
        Some(t) if t == *st.token => ws.on_upgrade(move |sock| ws_session(sock, st)),
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn ws_session(sock: WebSocket, st: AppState) {
    let (mut sink, mut stream) = sock.split();
    let mut rx = st.tx.subscribe();
    let hello = WsEvent::Hello { info: (*st.info).clone() };
    if sink.send(WsMsg::Text(serde_json::to_string(&hello).unwrap().into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let txt = serde_json::to_string(&ev).unwrap();
                    if sink.send(WsMsg::Text(txt.into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            msg = stream.next() => match msg {
                Some(Ok(WsMsg::Ping(p))) => { let _ = sink.send(WsMsg::Pong(p)).await; }
                Some(Ok(WsMsg::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            }
        }
    }
}

// ---------- watcher ----------

async fn watcher(st: AppState, every: Duration, db_path: PathBuf) {
    let mut last = match with_db(&st, |db, _| db.max_message_rowid()).await {
        Ok(v) => v,
        Err(e) => {
            error!("watcher init: {e:#}");
            0
        }
    };
    let wal = db_path.with_extension("db-wal");
    let mtime = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut last_wal = mtime(&wal);
    let mut last_chats_push = std::time::Instant::now();
    // Recently seen messages: rowid -> (fingerprint, first seen). Re-checked on every DB change so
    // that fields Messages.app fills in later (chat link, attachments, delivered/read, edits) get
    // pushed to clients too.
    let mut recent: HashMap<i64, (String, std::time::Instant)> = HashMap::new();
    const RECENT_WINDOW: Duration = Duration::from_secs(600);
    loop {
        tokio::time::sleep(every).await;
        if st.tx.receiver_count() == 0 {
            if let Ok(v) = with_db(&st, |db, _| db.max_message_rowid()).await {
                last = v;
            }
            last_wal = mtime(&wal);
            recent.clear();
            continue;
        }
        let now_max = match with_db(&st, |db, _| db.max_message_rowid()).await {
            Ok(v) => v,
            Err(e) => {
                warn!("watcher: {e:#}");
                continue;
            }
        };
        let mut changed = false;
        let mut to_push: Vec<Message> = vec![];
        if now_max > last {
            let since = last;
            match with_db(&st, move |db, book| {
                let msgs = db.messages_since(since, book)?;
                let unlinked = db.unlinked_since(since)?;
                Ok((msgs, unlinked))
            })
            .await
            {
                Ok((msgs, unlinked)) => {
                    let now = std::time::Instant::now();
                    for m in &msgs {
                        recent.insert(m.id, (String::new(), now));
                    }
                    for id in unlinked {
                        recent.insert(id, (String::new(), now));
                    }
                    to_push.extend(msgs);
                    changed = true;
                }
                Err(e) => warn!("watcher fetch: {e:#}"),
            }
            last = now_max;
        }
        let m = mtime(&wal);
        let wal_changed = m != last_wal;
        if wal_changed {
            last_wal = m;
            changed = true;
        }
        recent.retain(|_, (_, seen)| seen.elapsed() < RECENT_WINDOW);
        if (wal_changed || !to_push.is_empty()) && !recent.is_empty() {
            let ids: Vec<i64> = recent.keys().copied().collect();
            let ids2 = ids.clone();
            match with_db(&st, move |db, _| db.fingerprints(&ids2)).await {
                Ok(fps) => {
                    let mut dirty: Vec<i64> = vec![];
                    for (id, fp) in fps {
                        if let Some(entry) = recent.get_mut(&id) {
                            if entry.0 != fp {
                                let first = entry.0.is_empty();
                                entry.0 = fp;
                                // Newly pushed messages already carry the current state.
                                if !(first && to_push.iter().any(|m| m.id == id)) {
                                    dirty.push(id);
                                }
                            }
                        }
                    }
                    if !dirty.is_empty() {
                        match with_db(&st, move |db, book| db.messages_by_rowids(&dirty, book)).await {
                            Ok(msgs) => to_push.extend(msgs),
                            Err(e) => warn!("watcher recheck: {e:#}"),
                        }
                    }
                }
                Err(e) => warn!("watcher fingerprints: {e:#}"),
            }
        }
        if !to_push.is_empty() {
            // Dedupe by id, last write wins.
            let mut seen = std::collections::HashSet::new();
            let mut out = vec![];
            for m in to_push.into_iter().rev() {
                if seen.insert(m.id) {
                    out.push(m);
                }
            }
            out.reverse();
            let _ = st.tx.send(WsEvent::Messages { messages: out });
        }
        if changed && last_chats_push.elapsed() > Duration::from_millis(1500) {
            last_chats_push = std::time::Instant::now();
            if let Ok(chats) = with_db(&st, |db, book| db.chats(100, book)).await {
                let _ = st.tx.send(WsEvent::Chats { chats });
            }
        }
    }
}

async fn contacts_refresher(st: AppState) {
    loop {
        tokio::time::sleep(Duration::from_secs(300)).await;
        match tokio::task::spawn_blocking(contacts::load).await {
            Ok(Ok(book)) => {
                if let Ok(mut w) = st.contacts.write() {
                    *w = book;
                }
            }
            Ok(Err(e)) => warn!("contacts refresh: {e}"),
            Err(e) => warn!("contacts refresh join: {e}"),
        }
    }
}

// ---------- static web client ----------

static EMBEDDED: &[(&str, &str, &str)] = &[
    ("index.html", "text/html; charset=utf-8", include_str!("../../../web/index.html")),
    ("app.js", "application/javascript; charset=utf-8", include_str!("../../../web/app.js")),
    ("style.css", "text/css; charset=utf-8", include_str!("../../../web/style.css")),
];

async fn static_handler(State(st): State<AppState>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(dir) = &st.web_dir {
        if let Some(p) = resolve_web_asset(dir, path) {
            let mime = mime_guess::from_path(&p).first_or_octet_stream().to_string();
            return match tokio::fs::read(&p).await {
                Ok(b) => ([(header::CONTENT_TYPE, mime)], b).into_response(),
                Err(_) => StatusCode::NOT_FOUND.into_response(),
            };
        }
    }
    if let Some(response) = embedded_asset(path) {
        return response;
    }
    StatusCode::NOT_FOUND.into_response()
}

fn embedded_asset(path: &str) -> Option<Response> {
    EMBEDDED.iter().find(|(name, _, _)| *name == path).map(|(_, mime, body)| {
        ([(header::CONTENT_TYPE, *mime), (header::CACHE_CONTROL, "no-cache")], Body::from(*body)).into_response()
    })
}

fn resolve_web_asset(root: &std::path::Path, request_path: &str) -> Option<PathBuf> {
    let relative = std::path::Path::new(request_path);
    if relative.components().any(|component| matches!(component, std::path::Component::ParentDir)) {
        return None;
    }

    let canonical_root = std::fs::canonicalize(root).ok()?;
    let candidate = canonical_root.join(relative);
    let canonical_candidate = std::fs::canonicalize(candidate).ok()?;
    if !canonical_candidate.starts_with(&canonical_root) || !canonical_candidate.is_file() {
        return None;
    }
    Some(canonical_candidate)
}

#[cfg(test)]
mod tests {
    use super::{static_handler, AppState, Args};
    use axum::{body::to_bytes, extract::State};
    use clap::{CommandFactory, FromArgMatches};
    use imsg_core::ServerInfo;
    use rusqlite::Connection;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, RwLock};
    use tokio::sync::broadcast;

    #[test]
    fn web_asset_resolution_accepts_file_inside_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("app.js"), "ok").unwrap();
        let resolved = super::resolve_web_asset(dir.path(), "app.js").unwrap();
        assert_eq!(fs::read_to_string(resolved).unwrap(), "ok");
    }

    #[test]
    fn web_asset_resolution_rejects_parent_traversal() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("web");
        fs::create_dir(&root).unwrap();
        fs::write(parent.path().join("secret.txt"), "secret").unwrap();
        assert!(super::resolve_web_asset(&root, "../secret.txt").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn web_asset_resolution_rejects_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), root.path().join("asset.txt")).unwrap();
        assert!(super::resolve_web_asset(root.path(), "asset.txt").is_none());
    }

    #[test]
    fn web_asset_resolution_rejects_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        assert!(super::resolve_web_asset(dir.path(), "missing.css").is_none());
    }

    #[test]
    fn default_bind_is_all_interfaces() {
        let mut command = Args::command();
        command = command.mut_arg("bind", |arg| arg.env(None));
        let matches = command.try_get_matches_from(["imsg-server"]).unwrap();
        let args = Args::from_arg_matches(&matches).unwrap();
        assert_eq!(args.bind, "0.0.0.0:8787".parse().unwrap());
    }

    fn test_state(web_dir: PathBuf, db_dir: &std::path::Path) -> AppState {
        let db_path = db_dir.join("chat.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE handle (id TEXT)", []).unwrap();
        drop(conn);
        let db = super::Db::open(&db_path).unwrap();
        let (tx, _) = broadcast::channel(1);
        AppState {
            db: Arc::new(Mutex::new(db)),
            contacts: Arc::new(RwLock::new(super::ContactBook::default())),
            token: Arc::new("test".into()),
            tx,
            cache_dir: db_dir.join("cache"),
            upload_dir: db_dir.join("uploads"),
            info: Arc::new(ServerInfo { name: "test".into(), version: "test".into(), me: vec![], host: "test".into() }),
            web_dir: Some(web_dir),
        }
    }

    #[tokio::test]
    async fn missing_dev_asset_uses_embedded_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path().join("web"), dir.path());
        let response = static_handler(State(state), "/index.html".parse().unwrap()).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(body.windows("<!doctype html>".len()).any(|w| w.eq_ignore_ascii_case(b"<!doctype html>")));
    }

    #[tokio::test]
    async fn static_handler_rejects_parent_traversal() {
        let parent = tempfile::tempdir().unwrap();
        let web = parent.path().join("web");
        fs::create_dir(&web).unwrap();
        fs::write(parent.path().join("secret.txt"), "secret").unwrap();
        let state = test_state(web, parent.path());
        let response = static_handler(State(state), "/../secret.txt".parse().unwrap()).await;
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }
}
