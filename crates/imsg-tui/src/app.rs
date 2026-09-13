use crate::api::Api;
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use imsg_core::*;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use tokio::sync::mpsc;
use tui_textarea::TextArea;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Chats,
    Messages,
    Compose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    ChatFilter,
    Emoji,
    Contacts,
    Attach,
    Help,
    Search,
}

pub enum AppEvent {
    Info(ServerInfo),
    Chats(Vec<Chat>),
    Messages { chat_id: i64, page: MessagesPage, older: bool },
    Tail { chat_id: i64, messages: Vec<Message> },
    Contacts(Vec<Contact>),
    Sent(std::result::Result<SendResponse, String>),
    Uploaded(std::result::Result<UploadResponse, String>),
    Search(Vec<Message>),
    MessagesFailed { chat_id: i64, message: String },
    Ws(WsEvent),
    Status(String),
}

#[derive(Debug, Clone)]
pub struct NewChat {
    pub address: String,
    pub name: String,
}

pub struct App {
    pub api: Api,
    pub tx: mpsc::UnboundedSender<AppEvent>,
    pub info: Option<ServerInfo>,
    pub connected: bool,
    pub chats: Vec<Chat>,
    pub chat_sel: usize,
    pub chat_filter: String,
    pub messages: HashMap<i64, Vec<Message>>,
    pub has_more: HashMap<i64, bool>,
    pub loading: HashSet<i64>,
    pub scroll: usize,
    pub msg_sel: Option<usize>,
    pub compose: TextArea<'static>,
    pub pending_files: Vec<UploadResponse>,
    pub focus: Focus,
    pub mode: Mode,
    pub status: String,
    pub status_at: Instant,
    pub contacts: Vec<Contact>,
    pub picker_query: String,
    pub picker_sel: usize,
    pub emoji_query: String,
    pub emoji_sel: usize,
    pub attach_input: String,
    pub search_query: String,
    pub search_results: Vec<Message>,
    pub search_sel: usize,
    pub new_chat: Option<NewChat>,
    pub sending: bool,
    pub should_quit: bool,
    pub force_redraw: bool,
    /// Set by the renderer so key handling knows the viewport height.
    pub msg_view_height: usize,
    pub msg_total_lines: usize,
    /// (first_line, last_line) per rendered message index, filled by the renderer.
    pub msg_line_ranges: Vec<(usize, usize)>,
}

impl App {
    pub fn new(api: Api, tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        let mut compose = TextArea::default();
        compose.set_placeholder_text("iMessage  (Enter to send, Alt+Enter newline, Ctrl+E emoji, Ctrl+F file, ? help)");
        compose.set_cursor_line_style(ratatui::style::Style::default());
        App {
            api,
            tx,
            info: None,
            connected: false,
            chats: vec![],
            chat_sel: 0,
            chat_filter: String::new(),
            messages: HashMap::new(),
            has_more: HashMap::new(),
            loading: HashSet::new(),
            scroll: 0,
            msg_sel: None,
            compose,
            pending_files: vec![],
            focus: Focus::Chats,
            mode: Mode::Normal,
            status: "connecting…".into(),
            status_at: Instant::now(),
            contacts: vec![],
            picker_query: String::new(),
            picker_sel: 0,
            emoji_query: String::new(),
            emoji_sel: 0,
            attach_input: String::new(),
            search_query: String::new(),
            search_results: vec![],
            search_sel: 0,
            new_chat: None,
            sending: false,
            should_quit: false,
            force_redraw: false,
            msg_view_height: 20,
            msg_total_lines: 0,
            msg_line_ranges: vec![],
        }
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.status_at = Instant::now();
    }

    // ----- chat helpers -----

    pub fn filtered_chats(&self) -> Vec<usize> {
        if self.chat_filter.is_empty() {
            return (0..self.chats.len()).collect();
        }
        let q = self.chat_filter.to_lowercase();
        self.chats
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.title().to_lowercase().contains(&q)
                    || c.participants.iter().any(|p| p.address.to_lowercase().contains(&q))
                    || c.last_preview.as_deref().unwrap_or("").to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn current_chat(&self) -> Option<&Chat> {
        if self.new_chat.is_some() {
            return None;
        }
        let f = self.filtered_chats();
        f.get(self.chat_sel).and_then(|i| self.chats.get(*i))
    }

    pub fn current_chat_id(&self) -> Option<i64> {
        self.current_chat().map(|c| c.id)
    }

    pub fn current_messages(&self) -> &[Message] {
        match self.current_chat_id() {
            Some(id) => self.messages.get(&id).map(|v| v.as_slice()).unwrap_or(&[]),
            None => &[],
        }
    }

    // ----- async actions -----

    pub fn load_chats(&self) {
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.chats().await {
                Ok(c) => { let _ = tx.send(AppEvent::Chats(c)); }
                Err(e) => { let _ = tx.send(AppEvent::Status(format!("chats: {e}"))); }
            }
        });
    }

    pub fn load_info(&self) {
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.info().await {
                Ok(i) => { let _ = tx.send(AppEvent::Info(i)); }
                Err(e) => { let _ = tx.send(AppEvent::Status(format!("cannot reach server: {e}"))); }
            }
        });
    }

    pub fn load_contacts(&self) {
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.contacts().await {
                Ok(c) => { let _ = tx.send(AppEvent::Contacts(c)); }
                Err(e) => { let _ = tx.send(AppEvent::Status(format!("contacts: {e}"))); }
            }
        });
    }

    pub fn ensure_messages(&mut self) {
        let Some(id) = self.current_chat_id() else { return };
        if self.messages.contains_key(&id) || self.loading.contains(&id) {
            return;
        }
        self.loading.insert(id);
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.messages(id, None, 60).await {
                Ok(page) => { let _ = tx.send(AppEvent::Messages { chat_id: id, page, older: false }); }
                Err(e) => { let _ = tx.send(AppEvent::MessagesFailed { chat_id: id, message: format!("messages: {e}") }); }
            }
        });
    }

    pub fn load_older(&mut self) {
        let Some(id) = self.current_chat_id() else { return };
        if self.loading.contains(&id) || !self.has_more.get(&id).copied().unwrap_or(false) {
            return;
        }
        let Some(first) = self.messages.get(&id).and_then(|v| v.first()).map(|m| m.id) else { return };
        self.loading.insert(id);
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.messages(id, Some(first), 60).await {
                Ok(page) => { let _ = tx.send(AppEvent::Messages { chat_id: id, page, older: true }); }
                Err(e) => { let _ = tx.send(AppEvent::MessagesFailed { chat_id: id, message: format!("messages: {e}") }); }
            }
        });
    }

    /// If the chat list reports something newer than what we have, fetch the tail and merge.
    fn ensure_tail(&mut self) {
        let Some(c) = self.current_chat() else { return };
        let (id, last_date) = (c.id, c.last_date);
        let Some(list) = self.messages.get(&id) else { return };
        let newest = list.last().map(|m| m.date).unwrap_or(0);
        if last_date <= newest + 500 || self.loading.contains(&id) {
            return;
        }
        self.loading.insert(id);
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.messages(id, None, 30).await {
                Ok(page) => { let _ = tx.send(AppEvent::Tail { chat_id: id, messages: page.messages }); }
                Err(e) => { let _ = tx.send(AppEvent::MessagesFailed { chat_id: id, message: format!("messages: {e}") }); }
            }
        });
    }

    pub fn refresh_current(&mut self) {
        if let Some(id) = self.current_chat_id() {
            self.messages.remove(&id);
            self.ensure_messages();
        }
        self.load_chats();
    }

    fn send(&mut self) {
        let raw = self.compose.lines().join("\n");
        let text = expand_shortcodes(&raw);
        let text = text.trim().to_string();
        if text.is_empty() && self.pending_files.is_empty() {
            return;
        }
        let mut req = SendRequest { text: if text.is_empty() { None } else { Some(text) }, uploads: self.pending_files.iter().map(|u| u.id.clone()).collect(), ..Default::default() };
        if let Some(nc) = &self.new_chat {
            req.to = vec![nc.address.clone()];
        } else if let Some(c) = self.current_chat() {
            req.chat_guid = Some(c.guid.clone());
        } else {
            self.set_status("no conversation selected");
            return;
        }
        self.compose = fresh_compose();
        self.pending_files.clear();
        self.sending = true;
        self.set_status("sending…");
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = api.send(&req).await.map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::Sent(r));
        });
    }

    fn attach(&mut self, path: String) {
        let p = expand_tilde(path.trim());
        if !p.exists() {
            self.set_status(format!("no such file: {}", p.display()));
            return;
        }
        self.set_status(format!("uploading {}…", p.display()));
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = api.upload(&p).await.map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::Uploaded(r));
        });
    }

    fn open_selected_attachments(&mut self) {
        let Some(i) = self.msg_sel else { return };
        let msgs = self.current_messages();
        let Some(m) = msgs.get(i) else { return };
        let atts: Vec<Attachment> = m.attachments.iter().filter(|a| a.available).cloned().collect();
        let url = m.parts.iter().flat_map(|p| p.effects.iter()).find_map(|e| e.strip_prefix("link:").map(str::to_string))
            .or_else(|| if let MessageKind::App { url, .. } = &m.kind { url.clone() } else { None });
        if atts.is_empty() {
            if let Some(u) = url {
                let _ = std::process::Command::new("open").arg(&u).spawn();
                self.set_status(format!("opened {u}"));
            } else {
                self.set_status("nothing to open on this message");
            }
            return;
        }
        self.set_status(format!("downloading {} attachment(s)…", atts.len()));
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            for a in atts {
                match api.download(&a).await {
                    Ok(p) => {
                        let _ = std::process::Command::new("open").arg(&p).spawn();
                        let _ = tx.send(AppEvent::Status(format!("opened {}", p.display())));
                    }
                    Err(e) => { let _ = tx.send(AppEvent::Status(format!("open failed: {e}"))); }
                }
            }
        });
    }

    fn save_selected_attachments(&mut self) {
        let Some(i) = self.msg_sel else { return };
        let Some(m) = self.current_messages().get(i) else { return };
        let atts: Vec<Attachment> = m.attachments.iter().filter(|a| a.available).cloned().collect();
        if atts.is_empty() {
            self.set_status("no attachments on this message");
            return;
        }
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let home = std::env::var("HOME").unwrap_or_default();
            let dl = std::path::PathBuf::from(home).join("Downloads");
            for a in atts {
                match api.download(&a).await {
                    Ok(p) => {
                        let dest = dl.join(&a.name);
                        match std::fs::copy(&p, &dest) {
                            Ok(_) => { let _ = tx.send(AppEvent::Status(format!("saved {}", dest.display()))); }
                            Err(e) => { let _ = tx.send(AppEvent::Status(format!("save failed: {e}"))); }
                        }
                    }
                    Err(e) => { let _ = tx.send(AppEvent::Status(format!("download failed: {e}"))); }
                }
            }
        });
    }

    fn run_search(&mut self) {
        let q = self.search_query.trim().to_string();
        if q.is_empty() {
            return;
        }
        let api = self.api.clone();
        let tx = self.tx.clone();
        self.set_status(format!("searching \"{q}\"…"));
        tokio::spawn(async move {
            match api.search(&q).await {
                Ok(r) => { let _ = tx.send(AppEvent::Search(r)); }
                Err(e) => { let _ = tx.send(AppEvent::Status(format!("search: {e}"))); }
            }
        });
    }

    // ----- event handling -----

    pub fn on_app_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Info(i) => {
                self.info = Some(i);
                self.set_status("connected");
            }
            AppEvent::Chats(chats) => self.merge_chats(chats),
            AppEvent::Messages { chat_id, page, older } => {
                self.loading.remove(&chat_id);
                self.force_redraw = true;
                self.has_more.insert(chat_id, page.has_more);
                let entry = self.messages.entry(chat_id).or_default();
                if older {
                    let added = page.messages.len();
                    let mut v = page.messages;
                    v.extend(entry.drain(..));
                    *entry = v;
                    if let Some(s) = self.msg_sel.as_mut() {
                        *s += added;
                    }
                } else {
                    *entry = page.messages;
                }
            }
            AppEvent::MessagesFailed { chat_id, message } => {
                self.loading.remove(&chat_id);
                self.force_redraw = true;
                self.set_status(message);
            }
            AppEvent::Tail { chat_id, messages } => {
                self.loading.remove(&chat_id);
                let list = self.messages.entry(chat_id).or_default();
                for m in messages {
                    if let Some(pos) = list.iter().position(|x| x.id == m.id) { list[pos] = m; } else { list.push(m); }
                }
                list.sort_by_key(|x| x.id);
            }
            AppEvent::Contacts(c) => self.contacts = c,
            AppEvent::Sent(r) => {
                self.sending = false;
                match r {
                    Ok(SendResponse { ok: true, .. }) => self.set_status("sent"),
                    Ok(SendResponse { detail, .. }) => self.set_status(format!("send failed: {}", detail.unwrap_or_default())),
                    Err(e) => self.set_status(format!("send failed: {e}")),
                }
            }
            AppEvent::Uploaded(r) => match r {
                Ok(u) => {
                    self.set_status(format!("attached {} ({})", u.name, human_size(u.size as i64)));
                    self.pending_files.push(u);
                    self.focus = Focus::Compose;
                }
                Err(e) => self.set_status(format!("upload failed: {e}")),
            },
            AppEvent::Search(r) => {
                self.set_status(format!("{} result(s)", r.len()));
                self.search_results = r;
                self.search_sel = 0;
            }
            AppEvent::Status(s) => self.set_status(s),
            AppEvent::Ws(ev) => self.on_ws(ev),
        }
    }

    fn merge_chats(&mut self, chats: Vec<Chat>) {
        let cur = self.current_chat_id();
        // Chat websocket events are a limited recent window; retain older
        // conversations already loaded by the HTTP snapshot and merge by ID.
        let mut by_id: HashMap<i64, Chat> = self.chats.drain(..).map(|c| (c.id, c)).collect();
        for chat in chats {
            by_id.insert(chat.id, chat);
        }
        self.chats = by_id.into_values().collect();
        self.chats.sort_by(|a, b| b.last_date.cmp(&a.last_date).then_with(|| b.id.cmp(&a.id)));
        // Keep selection on the same chat.
        if let Some(id) = cur {
            let f = self.filtered_chats();
            if let Some(pos) = f.iter().position(|i| self.chats[*i].id == id) {
                self.chat_sel = pos;
            }
        }
        if self.new_chat.is_none() {
            self.ensure_messages();
            self.ensure_tail();
        }
        // A pending new conversation may now exist.
        if let Some(nc) = self.new_chat.clone() {
            let want = normalize_address(&nc.address);
            if let Some(pos) = self.chats.iter().position(|c| !c.is_group && c.participants.len() == 1 && normalize_address(&c.participants[0].address) == want) {
                self.new_chat = None;
                self.chat_filter.clear();
                self.chat_sel = pos;
                self.ensure_messages();
            }
        }
    }

    fn on_ws(&mut self, ev: WsEvent) {
        match ev {
            WsEvent::Hello { info } => {
                self.connected = true;
                self.info = Some(info);
                self.set_status("connected");
            }
            WsEvent::Messages { messages } => {
                let cur = self.current_chat_id();
                for m in messages {
                    let Some(list) = self.messages.get_mut(&m.chat_id) else {
                        // Do not turn a notification for an unopened chat into a
                        // misleading "loaded" empty history cache.
                        if cur != Some(m.chat_id) { continue; }
                        // The normal history request will include this message;
                        // leave the cache absent so that request can proceed.
                        self.ensure_messages();
                        continue;
                    };
                    if let Some(pos) = list.iter().position(|x| x.id == m.id) {
                        list[pos] = m;
                    } else {
                        list.push(m);
                        list.sort_by_key(|x| x.id);
                    }
                }
            }
            WsEvent::Chats { chats } => self.merge_chats(chats),
            WsEvent::Error { message } => {
                self.connected = false;
                self.set_status(message);
            }
        }
    }

    pub fn on_key(&mut self, ev: Event) -> Result<()> {
        let Event::Key(key) = ev.clone() else { return Ok(()) };
        if key.kind != crossterm::event::KeyEventKind::Press {
            return Ok(());
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Global keys
        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) | (KeyCode::Char('q'), true) => { self.should_quit = true; return Ok(()); }
            (KeyCode::Char('r'), true) => { self.refresh_current(); self.set_status("refreshed"); return Ok(()); }
            (KeyCode::Char('l'), true) => { self.force_redraw = true; return Ok(()); }
            (KeyCode::Char('n'), true) => { self.open_contacts(); return Ok(()); }
            (KeyCode::Char('e'), true) => { self.mode = Mode::Emoji; self.emoji_query.clear(); self.emoji_sel = 0; return Ok(()); }
            (KeyCode::Char('f'), true) => { self.mode = Mode::Attach; self.attach_input.clear(); return Ok(()); }
            (KeyCode::Char('s'), true) => { self.mode = Mode::Search; return Ok(()); }
            _ => {}
        }
        match self.mode {
            Mode::Help => { self.mode = Mode::Normal; Ok(()) }
            Mode::ChatFilter => self.key_chat_filter(key),
            Mode::Emoji => self.key_emoji(key),
            Mode::Contacts => self.key_contacts(key),
            Mode::Attach => self.key_attach(key),
            Mode::Search => self.key_search(key),
            Mode::Normal => match self.focus {
                Focus::Chats => self.key_chats(key),
                Focus::Messages => self.key_messages(key),
                Focus::Compose => self.key_compose(key, ev),
            },
        }
    }

    fn open_contacts(&mut self) {
        if self.contacts.is_empty() {
            self.load_contacts();
        }
        self.mode = Mode::Contacts;
        self.picker_query.clear();
        self.picker_sel = 0;
    }

    fn key_chats(&mut self, key: KeyEvent) -> Result<()> {
        let n = self.filtered_chats().len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => { if n > 0 { self.chat_sel = (self.chat_sel + 1).min(n - 1); } self.on_chat_changed(); }
            KeyCode::Char('k') | KeyCode::Up => { self.chat_sel = self.chat_sel.saturating_sub(1); self.on_chat_changed(); }
            KeyCode::Char('g') | KeyCode::Home => { self.chat_sel = 0; self.on_chat_changed(); }
            KeyCode::Char('G') | KeyCode::End => { if n > 0 { self.chat_sel = n - 1; } self.on_chat_changed(); }
            KeyCode::PageDown => { if n > 0 { self.chat_sel = (self.chat_sel + 10).min(n - 1); } self.on_chat_changed(); }
            KeyCode::PageUp => { self.chat_sel = self.chat_sel.saturating_sub(10); self.on_chat_changed(); }
            KeyCode::Enter | KeyCode::Char('i') => { self.focus = Focus::Compose; }
            KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => { self.focus = Focus::Messages; }
            KeyCode::Char('/') => { self.mode = Mode::ChatFilter; }
            KeyCode::Char('?') => { self.mode = Mode::Help; }
            KeyCode::Char('q') => { self.should_quit = true; }
            KeyCode::Esc => { if !self.chat_filter.is_empty() { self.chat_filter.clear(); self.chat_sel = 0; self.on_chat_changed(); } }
            _ => {}
        }
        Ok(())
    }

    fn on_chat_changed(&mut self) {
        self.new_chat = None;
        self.scroll = 0;
        self.msg_sel = None;
        self.force_redraw = true;
        self.ensure_messages();
    }

    fn key_messages(&mut self, key: KeyEvent) -> Result<()> {
        let n = self.current_messages().len();
        match key.code {
            KeyCode::Char('k') | KeyCode::Up => {
                match self.msg_sel {
                    None => { if n > 0 { self.msg_sel = Some(n - 1); } }
                    Some(0) => { self.load_older(); }
                    Some(i) => { self.msg_sel = Some(i - 1); }
                }
                self.scroll_to_selected();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                match self.msg_sel {
                    None => {}
                    Some(i) if i + 1 >= n => { self.msg_sel = None; self.scroll = 0; }
                    Some(i) => { self.msg_sel = Some(i + 1); }
                }
                self.scroll_to_selected();
            }
            KeyCode::PageUp | KeyCode::Char('u') => {
                self.msg_sel = None;
                let max = self.msg_total_lines.saturating_sub(self.msg_view_height);
                self.scroll = (self.scroll + self.msg_view_height / 2).min(max);
                if self.scroll >= max { self.load_older(); }
            }
            KeyCode::PageDown | KeyCode::Char('d') => {
                self.msg_sel = None;
                self.scroll = self.scroll.saturating_sub(self.msg_view_height / 2);
            }
            KeyCode::Char('g') | KeyCode::Home => { self.msg_sel = None; self.scroll = self.msg_total_lines.saturating_sub(self.msg_view_height); self.load_older(); }
            KeyCode::Char('G') | KeyCode::End => { self.msg_sel = None; self.scroll = 0; }
            KeyCode::Char('o') | KeyCode::Enter if self.msg_sel.is_some() => { self.open_selected_attachments(); }
            KeyCode::Char('s') if self.msg_sel.is_some() => { self.save_selected_attachments(); }
            KeyCode::Char('y') if self.msg_sel.is_some() => {
                if let Some(m) = self.msg_sel.and_then(|i| self.current_messages().get(i)) {
                    if let Some(t) = &m.text { copy_to_clipboard(t); self.set_status("copied"); }
                }
            }
            KeyCode::Enter | KeyCode::Char('i') => { self.focus = Focus::Compose; }
            KeyCode::Tab => { self.focus = Focus::Compose; }
            KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => { self.focus = Focus::Chats; }
            KeyCode::Esc => { if self.msg_sel.is_some() { self.msg_sel = None; } else { self.focus = Focus::Chats; } }
            KeyCode::Char('/') => { self.focus = Focus::Chats; self.mode = Mode::ChatFilter; }
            KeyCode::Char('?') => { self.mode = Mode::Help; }
            KeyCode::Char('q') => { self.should_quit = true; }
            _ => {}
        }
        Ok(())
    }

    /// Adjust scroll so the selected message is visible.
    pub fn scroll_to_selected(&mut self) {
        let Some(i) = self.msg_sel else { return };
        let Some(&(first, last)) = self.msg_line_ranges.get(i) else { return };
        let total = self.msg_total_lines;
        let h = self.msg_view_height.max(1);
        // Visible window in line indices: [total - h - scroll, total - scroll)
        let top = total.saturating_sub(h + self.scroll);
        let bottom = total.saturating_sub(self.scroll);
        if first < top {
            self.scroll = total.saturating_sub(h + first);
        } else if last >= bottom {
            self.scroll = total.saturating_sub(last + 1);
        }
    }

    fn key_compose(&mut self, key: KeyEvent, ev: Event) -> Result<()> {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => { self.focus = Focus::Messages; }
            KeyCode::Enter if alt || shift => { self.compose.insert_newline(); }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => { self.compose.insert_newline(); }
            KeyCode::Enter => { self.send(); }
            KeyCode::Tab => { self.focus = Focus::Chats; }
            KeyCode::BackTab => { self.focus = Focus::Messages; }
            KeyCode::Up if self.compose.lines().len() <= 1 && self.compose.lines()[0].is_empty() => { self.focus = Focus::Messages; }
            KeyCode::Backspace if self.compose.lines().join("").is_empty() && !self.pending_files.is_empty() => { self.pending_files.pop(); }
            _ => { self.compose.input(ev); self.maybe_expand_shortcode(); }
        }
        Ok(())
    }

    /// When the user finishes typing `:shortcode:` replace it inline with the emoji.
    fn maybe_expand_shortcode(&mut self) {
        let (row, col) = self.compose.cursor();
        let line = match self.compose.lines().get(row) { Some(l) => l.clone(), None => return };
        let before: String = line.chars().take(col).collect();
        if !before.ends_with(':') {
            return;
        }
        let trimmed = &before[..before.len() - 1];
        let Some(start) = trimmed.rfind(':') else { return };
        let code = &trimmed[start + 1..];
        if code.is_empty() || code.contains(' ') {
            return;
        }
        if let Some(e) = emojis::get_by_shortcode(code) {
            let n = code.chars().count() + 2;
            for _ in 0..n {
                self.compose.delete_char();
            }
            self.compose.insert_str(e.as_str());
        }
    }

    fn key_chat_filter(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => { self.chat_filter.clear(); self.mode = Mode::Normal; self.chat_sel = 0; self.on_chat_changed(); }
            KeyCode::Enter | KeyCode::Down | KeyCode::Tab => { self.mode = Mode::Normal; self.focus = Focus::Chats; }
            KeyCode::Backspace => { self.chat_filter.pop(); self.chat_sel = 0; self.on_chat_changed(); }
            KeyCode::Char(c) => { self.chat_filter.push(c); self.chat_sel = 0; self.on_chat_changed(); }
            _ => {}
        }
        Ok(())
    }

    pub fn emoji_matches(&self) -> Vec<&'static emojis::Emoji> {
        let q = self.emoji_query.to_lowercase();
        let mut out: Vec<&'static emojis::Emoji> = vec![];
        let mut exact: Vec<&'static emojis::Emoji> = vec![];
        for e in emojis::iter() {
            if q.is_empty() {
                out.push(e);
            } else if e.shortcodes().any(|s| s == q) {
                exact.push(e);
            } else if e.name().to_lowercase().contains(&q) || e.shortcodes().any(|s| s.contains(&q)) {
                out.push(e);
            }
            if out.len() + exact.len() > 400 {
                break;
            }
        }
        exact.extend(out);
        exact
    }

    fn key_emoji(&mut self, key: KeyEvent) -> Result<()> {
        let n = self.emoji_matches().len();
        match key.code {
            KeyCode::Esc => { self.mode = Mode::Normal; }
            KeyCode::Enter => {
                if let Some(e) = self.emoji_matches().get(self.emoji_sel).copied() {
                    self.compose.insert_str(e.as_str());
                    self.mode = Mode::Normal;
                    self.focus = Focus::Compose;
                }
            }
            KeyCode::Down | KeyCode::Tab => { if n > 0 { self.emoji_sel = (self.emoji_sel + 1) % n; } }
            KeyCode::Up | KeyCode::BackTab => { if n > 0 { self.emoji_sel = (self.emoji_sel + n - 1) % n; } }
            KeyCode::Backspace => { self.emoji_query.pop(); self.emoji_sel = 0; }
            KeyCode::Char(c) => { self.emoji_query.push(c); self.emoji_sel = 0; }
            _ => {}
        }
        Ok(())
    }

    pub fn contact_matches(&self) -> Vec<(String, String, Option<i64>)> {
        // (display name, address, contact id)
        let q = self.picker_query.to_lowercase();
        let mut out = vec![];
        for c in &self.contacts {
            let hit = q.is_empty() || c.name.to_lowercase().contains(&q) || c.phones.iter().chain(c.emails.iter()).any(|a| a.to_lowercase().contains(&q));
            if !hit {
                continue;
            }
            for p in &c.phones {
                out.push((c.name.clone(), p.clone(), Some(c.id)));
            }
            for e in &c.emails {
                out.push((c.name.clone(), e.clone(), Some(c.id)));
            }
            if out.len() > 200 {
                break;
            }
        }
        // Allow raw phone/email entry.
        let raw = self.picker_query.trim();
        if !raw.is_empty() && (raw.contains('@') || raw.chars().filter(|c| c.is_ascii_digit()).count() >= 7) {
            out.insert(0, (format!("Send to \"{raw}\""), raw.to_string(), None));
        }
        out
    }

    fn key_contacts(&mut self, key: KeyEvent) -> Result<()> {
        let matches = self.contact_matches();
        match key.code {
            KeyCode::Esc => { self.mode = Mode::Normal; }
            KeyCode::Enter => {
                if let Some((name, addr, _)) = matches.get(self.picker_sel).cloned() {
                    self.start_conversation(name, addr);
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Down | KeyCode::Tab => { if !matches.is_empty() { self.picker_sel = (self.picker_sel + 1) % matches.len(); } }
            KeyCode::Up | KeyCode::BackTab => { if !matches.is_empty() { self.picker_sel = (self.picker_sel + matches.len() - 1) % matches.len(); } }
            KeyCode::Backspace => { self.picker_query.pop(); self.picker_sel = 0; }
            KeyCode::Char(c) => { self.picker_query.push(c); self.picker_sel = 0; }
            _ => {}
        }
        Ok(())
    }

    fn start_conversation(&mut self, name: String, address: String) {
        let want = normalize_address(&address);
        self.chat_filter.clear();
        if let Some(pos) = self.chats.iter().position(|c| !c.is_group && c.participants.len() == 1 && normalize_address(&c.participants[0].address) == want) {
            self.new_chat = None;
            self.chat_sel = pos;
            self.on_chat_changed();
        } else {
            let name = if name.starts_with("Send to") { address.clone() } else { name };
            self.new_chat = Some(NewChat { address, name });
            self.scroll = 0;
            self.msg_sel = None;
        }
        self.focus = Focus::Compose;
    }

    fn key_attach(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => { self.mode = Mode::Normal; }
            KeyCode::Enter => {
                let p = self.attach_input.clone();
                self.mode = Mode::Normal;
                if !p.trim().is_empty() {
                    self.attach(p);
                }
            }
            KeyCode::Backspace => { self.attach_input.pop(); }
            KeyCode::Tab => {
                // Simple path completion.
                if let Some(c) = complete_path(&self.attach_input) { self.attach_input = c; }
            }
            KeyCode::Char(c) => { self.attach_input.push(c); }
            _ => {}
        }
        Ok(())
    }

    fn key_search(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => { self.mode = Mode::Normal; }
            KeyCode::Enter => {
                if self.search_results.is_empty() {
                    self.run_search();
                } else if let Some(m) = self.search_results.get(self.search_sel).cloned() {
                    // Jump to that chat.
                    self.chat_filter.clear();
                    if let Some(pos) = self.chats.iter().position(|c| c.id == m.chat_id) {
                        self.chat_sel = pos;
                        self.on_chat_changed();
                        self.focus = Focus::Messages;
                    }
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Down | KeyCode::Tab => { if !self.search_results.is_empty() { self.search_sel = (self.search_sel + 1) % self.search_results.len(); } }
            KeyCode::Up | KeyCode::BackTab => { if !self.search_results.is_empty() { self.search_sel = (self.search_sel + self.search_results.len() - 1) % self.search_results.len(); } }
            KeyCode::Backspace => { self.search_query.pop(); self.search_results.clear(); }
            KeyCode::Char(c) => { self.search_query.push(c); self.search_results.clear(); }
            _ => {}
        }
        Ok(())
    }
}

fn fresh_compose() -> TextArea<'static> {
    let mut t = TextArea::default();
    t.set_placeholder_text("iMessage  (Enter to send, Alt+Enter newline, Ctrl+E emoji, Ctrl+F file, ? help)");
    t.set_cursor_line_style(ratatui::style::Style::default());
    t
}

pub fn expand_shortcodes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(':') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find(':') {
            let code = &after[..j];
            if !code.is_empty() && !code.contains(char::is_whitespace) {
                if let Some(e) = emojis::get_by_shortcode(code) {
                    out.push_str(e.as_str());
                    rest = &after[j + 1..];
                    continue;
                }
            }
        }
        out.push(':');
        rest = after;
    }
    out.push_str(rest);
    out
}

pub fn expand_tilde(p: &str) -> std::path::PathBuf {
    if let Some(r) = p.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        return std::path::PathBuf::from(home).join(r);
    }
    std::path::PathBuf::from(p)
}

fn complete_path(input: &str) -> Option<String> {
    let p = expand_tilde(input);
    let (dir, prefix) = if input.ends_with('/') { (p.clone(), String::new()) } else { (p.parent()?.to_path_buf(), p.file_name()?.to_string_lossy().to_string()) };
    let mut matches: Vec<_> = std::fs::read_dir(&dir).ok()?.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&prefix)).collect();
    matches.sort_by_key(|e| e.file_name());
    let e = matches.first()?;
    let mut s = e.path().to_string_lossy().to_string();
    if e.path().is_dir() {
        s.push('/');
    }
    Some(s)
}

pub fn human_size(b: i64) -> String {
    let b = b as f64;
    if b > 1e9 { format!("{:.1} GB", b / 1e9) } else if b > 1e6 { format!("{:.1} MB", b / 1e6) } else if b > 1e3 { format!("{:.0} KB", b / 1e3) } else { format!("{b} B") }
}

fn copy_to_clipboard(s: &str) {
    use std::io::Write;
    if let Ok(mut c) = std::process::Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() {
        if let Some(mut i) = c.stdin.take() {
            let _ = i.write_all(s.as_bytes());
        }
        let _ = c.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn app() -> App {
        let (tx, _rx) = mpsc::unbounded_channel();
        App::new(Api::new(Config { server: "http://127.0.0.1:1".into(), token: "test".into() }), tx)
    }

    fn chat(id: i64, date: i64, preview: &str) -> Chat {
        Chat { id, guid: format!("g{id}"), identifier: format!("c{id}"), service: "iMessage".into(), display_name: Some(format!("Chat {id}")), is_group: false, participants: vec![], last_date: date, last_preview: Some(preview.into()), last_from_me: false, unread: 0 }
    }

    fn message(id: i64, chat_id: i64) -> Message {
        Message { id, guid: format!("m{id}"), chat_id, sender: None, is_from_me: false, date: id, date_read: 0, date_delivered: 0, is_read: false, is_delivered: false, is_sent: true, error: 0, service: "iMessage".into(), subject: None, text: Some("hello".into()), parts: vec![], attachments: vec![], reactions: vec![], kind: MessageKind::Normal, reply_to: None, thread_originator_guid: None, is_edited: false, expressive: None, is_audio: false }
    }

    #[tokio::test]
    async fn ws_message_does_not_cache_unopened_chat_and_loaded_empty_chat_updates() {
        let mut app = app();
        app.on_app_event(AppEvent::Ws(WsEvent::Messages { messages: vec![message(1, 7)] }));
        assert!(!app.messages.contains_key(&7));
        app.chats = vec![chat(7, 1, "")];
        app.ensure_messages();
        assert!(app.loading.contains(&7));
        app.loading.remove(&7);
        app.messages.insert(7, vec![]);
        app.chats.push(chat(8, 2, "other"));
        app.chat_sel = 1;
        app.on_app_event(AppEvent::Ws(WsEvent::Messages { messages: vec![message(1, 7)] }));
        assert_eq!(app.messages.get(&7).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn message_failure_clears_loading_for_retry() {
        let mut app = app();
        app.chats = vec![chat(1, 1000, "")];
        app.messages.insert(1, vec![message(1, 1)]);
        app.has_more.insert(1, true);
        app.loading.insert(1);
        app.loading.insert(2);
        app.on_app_event(AppEvent::MessagesFailed { chat_id: 1, message: "messages: failed".into() });
        app.load_older();
        assert!(app.loading.contains(&1));
        app.on_app_event(AppEvent::MessagesFailed { chat_id: 1, message: "older: failed".into() });
        app.ensure_tail();
        assert!(app.loading.contains(&1));
        app.on_app_event(AppEvent::MessagesFailed { chat_id: 1, message: "tail: failed".into() });
        assert!(!app.loading.contains(&1));
        assert!(app.loading.contains(&2));
        app.messages.remove(&1);
        app.ensure_messages();
        assert!(app.loading.contains(&1));
    }

    #[tokio::test]
    async fn limited_chat_updates_merge_and_preserve_selection() {
        let mut app = app();
        app.chats = vec![chat(1, 100, "old"), chat(2, 300, "selected")];
        app.chat_sel = 1;
        app.merge_chats(vec![chat(1, 500, "updated"), chat(3, 400, "new")]);
        assert_eq!(app.chats.iter().map(|c| c.id).collect::<Vec<_>>(), vec![1, 3, 2]);
        assert_eq!(app.current_chat_id(), Some(2));
        assert_eq!(app.chats[0].last_preview.as_deref(), Some("updated"));
    }
}
