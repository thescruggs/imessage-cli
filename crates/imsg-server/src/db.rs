//! Read the Messages chat.db and convert rows to wire types.
use crate::contacts::ContactBook;
use anyhow::{Context, Result};
use imessage_database::{
    message_types::{
        edited::EditStatus,
        expressives::{BubbleEffect, Expressive, ScreenEffect},
        text_effects::{animation::Animation, style::Style, text_effect::TextEffect},
        url::URLMessage,
        variants::{BalloonProvider, CustomBalloon, Tapback, TapbackAction, Variant},
    },
    tables::{
        attachment::Attachment as DbAttachment,
        messages::{message::Message as DbMessage, models::{BubbleComponent, GroupAction}},
        table::Table,
    },
    util::plist::parse_ns_keyed_archiver,
};
use imsg_core::*;
use rusqlite::{params, Connection, OpenFlags, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MSG_COLS: &str = "m.ROWID as rowid, m.guid, m.text, m.service, m.handle_id, m.destination_caller_id, m.subject, m.date, m.date_read, m.date_delivered, m.is_from_me, m.is_read, m.item_type, m.other_handle, m.share_status, m.share_direction, m.group_title, m.group_action_type, m.associated_message_guid, m.associated_message_type, m.balloon_bundle_id, m.expressive_send_style_id, m.thread_originator_guid, m.thread_originator_part, m.date_edited, m.associated_message_emoji, \
 c.chat_id, (SELECT COUNT(*) FROM message_attachment_join a WHERE m.ROWID = a.message_id) as num_attachments, NULL as deleted_from, 0 as num_replies, \
 m.is_delivered, m.is_sent, m.error, m.is_audio_message, m.date_retracted";

const NOT_TAPBACK: &str = "(m.associated_message_type IS NULL OR m.associated_message_type = 0 OR m.associated_message_type = 2 OR m.associated_message_type = 3)";

pub struct Db {
    conn: Connection,
    handles: HashMap<i64, String>,
    handles_loaded_max: i64,
    /// Highest message ROWID a client has viewed, per chat. Messages.app owns
    /// the real `is_read` flag and nothing outside it can set that, so imsg
    /// keeps its own record and subtracts it from the unread count.
    seen: HashMap<i64, i64>,
    seen_path: Option<PathBuf>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("open {}", path.display()))?;
        let _ = conn.pragma_update(None, "cache_size", -32_768_i64);
        let mut db = Db { conn, handles: HashMap::new(), handles_loaded_max: 0, seen: HashMap::new(), seen_path: None };
        db.refresh_handles()?;
        Ok(db)
    }

    /// Persist "seen" marks in `path` (JSON `{ "<chat_id>": <message_id> }`), loading any existing file.
    pub fn with_seen_file(mut self, path: PathBuf) -> Self {
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<HashMap<String, i64>>(&s) {
                Ok(m) => self.seen = m.into_iter().filter_map(|(k, v)| k.parse().ok().map(|k| (k, v))).collect(),
                Err(e) => tracing::warn!("ignoring unreadable {}: {e}", path.display()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("cannot read {}: {e}", path.display()),
        }
        self.seen_path = Some(path);
        self
    }

    /// Record that everything in `chat_id` up to `message_id` (or the newest
    /// message when `None`) has been viewed. Returns the id that was recorded.
    pub fn mark_seen(&mut self, chat_id: i64, message_id: Option<i64>) -> Result<i64> {
        let id = match message_id {
            Some(id) => id,
            None => self.conn.query_row(
                "SELECT COALESCE(MAX(message_id), 0) FROM chat_message_join WHERE chat_id = ?1",
                [chat_id],
                |r| r.get(0),
            )?,
        };
        let current = self.seen.get(&chat_id).copied().unwrap_or(0);
        if id > current {
            self.seen.insert(chat_id, id);
            self.save_seen()?;
            return Ok(id);
        }
        Ok(current)
    }

    fn save_seen(&self) -> Result<()> {
        let Some(path) = &self.seen_path else { return Ok(()) };
        let map: HashMap<String, i64> = self.seen.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&map)?).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
        Ok(())
    }

    fn refresh_handles(&mut self) -> Result<()> {
        let mut st = self.conn.prepare_cached("SELECT ROWID, id FROM handle WHERE ROWID > ?1")?;
        let rows = st.query_map([self.handles_loaded_max], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for (id, addr) in rows.flatten() {
            self.handles_loaded_max = self.handles_loaded_max.max(id);
            self.handles.insert(id, addr);
        }
        Ok(())
    }

    fn participant(&self, handle_id: i64, book: &ContactBook) -> Option<Participant> {
        let address = self.handles.get(&handle_id)?.clone();
        let c = book.lookup(&address);
        Some(Participant {
            handle_id,
            address,
            name: c.map(|c| c.name.clone()),
            contact_id: c.map(|c| c.id),
        })
    }

    pub fn max_message_rowid(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COALESCE(MAX(ROWID),0) FROM message", [], |r| r.get(0))?)
    }

    /// Own addresses (phone/email) seen as destination_caller_id.
    pub fn my_addresses(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(
            "SELECT destination_caller_id, COUNT(*) c FROM message WHERE is_from_me = 1 AND destination_caller_id IS NOT NULL GROUP BY 1 ORDER BY c DESC LIMIT 5",
        )?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut out: Vec<String> = vec![];
        for a in rows.flatten() {
            let a = a.trim_start_matches("tel:").to_string();
            if !out.iter().any(|x| normalize_address(x) == normalize_address(&a)) {
                out.push(a);
            }
        }
        Ok(out)
    }

    pub fn chat_by_id(&mut self, id: i64, book: &ContactBook) -> Result<Option<Chat>> {
        let mut v = self.chats_where("WHERE c.ROWID = ?1", params![id], book)?;
        Ok(v.pop())
    }

    /// Find an existing chat for a set of addresses (1:1 only when one address).
    pub fn chat_for_addresses(&mut self, addrs: &[String], book: &ContactBook) -> Result<Option<Chat>> {
        if addrs.len() != 1 {
            return Ok(None);
        }
        let want = normalize_address(&addrs[0]);
        let all = self.chats(500, book)?;
        Ok(all.into_iter().find(|c| !c.is_group && c.participants.len() == 1 && normalize_address(&c.participants[0].address) == want))
    }

    pub fn chats(&mut self, limit: i64, book: &ContactBook) -> Result<Vec<Chat>> {
        self.chats_where("ORDER BY last_date DESC LIMIT ?1", params![limit], book)
    }

    fn chats_where(&mut self, tail: &str, p: impl rusqlite::Params, book: &ContactBook) -> Result<Vec<Chat>> {
        self.refresh_handles()?;
        let sql = format!(
            "SELECT c.ROWID, c.guid, c.chat_identifier, c.service_name, c.display_name, c.style, \
             (SELECT MAX(j.message_date) FROM chat_message_join j WHERE j.chat_id = c.ROWID) AS last_date \
             FROM chat c {tail}"
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st.query_map(p, |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<i64>>(6)?,
            ))
        })?;
        let raw: Vec<_> = rows.flatten().collect();
        drop(st);
        let mut out = Vec::with_capacity(raw.len());
        for (id, guid, identifier, service, display_name, style, last_date) in raw {
            let Some(last_date) = last_date else { continue };
            let participants = self.chat_participants(id, book)?;
            let (last_preview, last_from_me) = self.last_preview(id)?;
            let seen = self.seen.get(&id).copied().unwrap_or(0);
            let unread: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM message m JOIN chat_message_join j ON j.message_id = m.ROWID WHERE j.chat_id = ?1 AND m.ROWID > ?2 AND m.is_from_me = 0 AND m.is_read = 0 AND m.item_type = 0 AND (m.associated_message_type IS NULL OR m.associated_message_type < 1000)",
                params![id, seen],
                |r| r.get(0),
            )?;
            out.push(Chat {
                id,
                guid,
                identifier,
                service: service.unwrap_or_else(|| "iMessage".into()),
                display_name: display_name.filter(|s| !s.is_empty()),
                is_group: style == 43,
                participants,
                last_date: apple_ns_to_unix_ms(last_date),
                last_preview,
                last_from_me,
                unread,
            });
        }
        Ok(out)
    }

    fn chat_participants(&self, chat_id: i64, book: &ContactBook) -> Result<Vec<Participant>> {
        let mut st = self.conn.prepare_cached("SELECT handle_id FROM chat_handle_join WHERE chat_id = ?1")?;
        let ids: Vec<i64> = st.query_map([chat_id], |r| r.get(0))?.flatten().collect();
        let mut seen = std::collections::HashSet::new();
        let mut out = vec![];
        for h in ids {
            if let Some(p) = self.participant(h, book) {
                if seen.insert(normalize_address(&p.address)) {
                    out.push(p);
                }
            }
        }
        Ok(out)
    }

    fn last_preview(&self, chat_id: i64) -> Result<(Option<String>, bool)> {
        let sql = format!(
            "SELECT {MSG_COLS} FROM message m JOIN chat_message_join c ON c.message_id = m.ROWID \
             WHERE c.chat_id = ?1 AND {NOT_TAPBACK} ORDER BY m.date DESC LIMIT 1"
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let mut rows = st.query([chat_id])?;
        if let Some(row) = rows.next()? {
            let mut m = DbMessage::from_row(row)?;
            let from_me = m.is_from_me;
            if let Ok(b) = m.parse_body(&self.conn) {
                m.apply_body(b);
            }
            let n_att = m.num_attachments;
            let text = m.text.clone().map(|t| clean_text(&t)).filter(|t| !t.is_empty());
            let preview = match (text, n_att) {
                (Some(t), _) => Some(t),
                (None, n) if n > 0 => Some("📎 Attachment".into()),
                _ => match m.get_announcement() {
                    Some(_) => Some("(system message)".into()),
                    None => if m.balloon_bundle_id.is_some() { Some("🔗 Link".into()) } else { None },
                },
            };
            return Ok((preview, from_me));
        }
        Ok((None, false))
    }

    pub fn messages(&mut self, chat_id: i64, before_id: Option<i64>, limit: i64, book: &ContactBook) -> Result<MessagesPage> {
        self.refresh_handles()?;
        let before = before_id.unwrap_or(i64::MAX);
        let sql = format!(
            "SELECT {MSG_COLS} FROM message m JOIN chat_message_join c ON c.message_id = m.ROWID \
             WHERE c.chat_id = ?1 AND m.ROWID < ?2 AND {NOT_TAPBACK} ORDER BY m.ROWID DESC LIMIT ?3"
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let mut rows = st.query(params![chat_id, before, limit + 1])?;
        let mut raw: Vec<(DbMessage, Extra)> = vec![];
        while let Some(row) = rows.next()? {
            raw.push((DbMessage::from_row(row)?, Extra::from_row(row)?));
        }
        drop(rows);
        drop(st);
        let has_more = raw.len() as i64 > limit;
        raw.truncate(limit as usize);
        raw.reverse();
        let mut msgs = Vec::with_capacity(raw.len());
        for (m, extra) in raw {
            msgs.push(self.convert(m, extra, book)?);
        }
        self.attach_reactions(&mut msgs, book)?;
        Ok(MessagesPage { messages: msgs, has_more })
    }

    /// Cheap change-detection fingerprint per message: anything Messages.app fills in after
    /// the row first appears (chat link, attachments, sent/delivered/read, edits, unsend).
    pub fn fingerprints(&self, ids: &[i64]) -> Result<Vec<(i64, String)>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT m.ROWID, COALESCE(c.chat_id,0) || ':' || m.is_sent || ':' || m.is_delivered || ':' || m.date_delivered || ':' || m.date_read || ':' || m.is_read || ':' || m.error || ':' || m.date_edited || ':' || m.date_retracted || ':' || COALESCE(length(m.attributedBody),0) || ':' || COALESCE(length(m.text),0) || ':' || \
             (SELECT COUNT(*) || '/' || COALESCE(SUM(a.transfer_state),0) FROM message_attachment_join j JOIN attachment a ON a.ROWID = j.attachment_id WHERE j.message_id = m.ROWID) \
             FROM message m LEFT JOIN chat_message_join c ON c.message_id = m.ROWID WHERE m.ROWID IN ({list})"
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.flatten().collect())
    }

    /// Fetch specific messages (skipping ones that still have no chat link).
    pub fn messages_by_rowids(&mut self, ids: &[i64], book: &ContactBook) -> Result<Vec<Message>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        self.refresh_handles()?;
        let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT {MSG_COLS} FROM message m LEFT JOIN chat_message_join c ON c.message_id = m.ROWID WHERE m.ROWID IN ({list}) ORDER BY m.ROWID"
        );
        let mut st = self.conn.prepare(&sql)?;
        let mut rows = st.query([])?;
        let mut raw = vec![];
        while let Some(row) = rows.next()? {
            raw.push((DbMessage::from_row(row)?, Extra::from_row(row)?));
        }
        drop(rows);
        drop(st);
        let mut out = vec![];
        for (m, e) in raw {
            if m.chat_id.is_none() || matches!(m.associated_message_type, Some(1000..=3999)) {
                continue;
            }
            out.push(self.convert(m, e, book)?);
        }
        self.attach_reactions_multi(&mut out, book)?;
        Ok(out)
    }

    /// attach_reactions assumes a single chat; group by chat first.
    fn attach_reactions_multi(&self, msgs: &mut [Message], book: &ContactBook) -> Result<()> {
        let mut by_chat: HashMap<i64, Vec<usize>> = HashMap::new();
        for (i, m) in msgs.iter().enumerate() {
            by_chat.entry(m.chat_id).or_default().push(i);
        }
        for (_, idxs) in by_chat {
            let mut group: Vec<Message> = idxs.iter().map(|i| msgs[*i].clone()).collect();
            self.attach_reactions(&mut group, book)?;
            for (k, i) in idxs.into_iter().enumerate() {
                msgs[i] = group[k].clone();
            }
        }
        Ok(())
    }

    /// Row ids > since that have no chat link yet (Messages.app writes the join a beat later).
    pub fn unlinked_since(&self, since: i64) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare_cached(
            "SELECT m.ROWID FROM message m LEFT JOIN chat_message_join c ON c.message_id = m.ROWID WHERE m.ROWID > ?1 AND c.chat_id IS NULL",
        )?;
        let rows = st.query_map([since], |r| r.get::<_, i64>(0))?;
        Ok(rows.flatten().collect())
    }

    /// All messages with ROWID > since (any chat), plus targets of any new tapbacks.
    pub fn messages_since(&mut self, since: i64, book: &ContactBook) -> Result<Vec<Message>> {
        self.refresh_handles()?;
        let sql = format!(
            "SELECT {MSG_COLS} FROM message m LEFT JOIN chat_message_join c ON c.message_id = m.ROWID \
             WHERE m.ROWID > ?1 ORDER BY m.ROWID ASC LIMIT 500"
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let mut rows = st.query([since])?;
        let mut raw: Vec<(DbMessage, Extra)> = vec![];
        while let Some(row) = rows.next()? {
            raw.push((DbMessage::from_row(row)?, Extra::from_row(row)?));
        }
        drop(rows);
        drop(st);
        let mut out = vec![];
        let mut tapback_targets: Vec<String> = vec![];
        for (m, extra) in raw {
            if matches!(m.associated_message_type, Some(1000..=3999)) {
                if let Some((_, g)) = m.clean_associated_guid() {
                    tapback_targets.push(g.to_string());
                }
                continue;
            }
            if m.chat_id.is_none() {
                continue;
            }
            out.push(self.convert(m, extra, book)?);
        }
        for g in tapback_targets {
            if let Some(m) = self.message_by_guid(&g, book)? {
                out.push(m);
            }
        }
        self.attach_reactions_multi(&mut out, book)?;
        Ok(out)
    }

    pub fn message_by_guid(&mut self, guid: &str, book: &ContactBook) -> Result<Option<Message>> {
        let sql = format!(
            "SELECT {MSG_COLS} FROM message m LEFT JOIN chat_message_join c ON c.message_id = m.ROWID WHERE m.guid = ?1 LIMIT 1"
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let mut rows = st.query([guid])?;
        let found = match rows.next()? {
            Some(row) => Some((DbMessage::from_row(row)?, Extra::from_row(row)?)),
            None => None,
        };
        drop(rows);
        drop(st);
        match found {
            Some((m, e)) if m.chat_id.is_some() => Ok(Some(self.convert(m, e, book)?)),
            _ => Ok(None),
        }
    }

    pub fn search(&mut self, q: &str, limit: i64, book: &ContactBook) -> Result<Vec<Message>> {
        self.refresh_handles()?;
        let sql = format!(
            "SELECT {MSG_COLS} FROM message m JOIN chat_message_join c ON c.message_id = m.ROWID \
             WHERE m.text LIKE ?1 AND {NOT_TAPBACK} ORDER BY m.date DESC LIMIT ?2"
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let pat = format!("%{}%", q.replace('%', "\\%"));
        let mut rows = st.query(params![pat, limit])?;
        let mut raw = vec![];
        while let Some(row) = rows.next()? {
            raw.push((DbMessage::from_row(row)?, Extra::from_row(row)?));
        }
        drop(rows);
        drop(st);
        let mut out = vec![];
        for (m, e) in raw {
            out.push(self.convert(m, e, book)?);
        }
        Ok(out)
    }

    pub fn attachment_path(&self, id: i64) -> Result<Option<(PathBuf, String, String)>> {
        let mut st = self.conn.prepare_cached("SELECT filename, mime_type, transfer_name, uti FROM attachment WHERE ROWID = ?1")?;
        let r = st.query_row([id], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        });
        match r {
            Ok((Some(f), mime, name, uti)) => {
                let p = expand_home(&f);
                let name = name.unwrap_or_else(|| p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                let mime = mime.or_else(|| mime_guess::from_path(&p).first().map(|m| m.to_string()))
                    .or_else(|| uti_to_mime(uti.as_deref()))
                    .unwrap_or_else(|| "application/octet-stream".into());
                Ok(Some((p, mime, name)))
            }
            Ok(_) => Ok(None),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn attach_reactions(&self, msgs: &mut [Message], book: &ContactBook) -> Result<()> {
        if msgs.is_empty() {
            return Ok(());
        }
        let chat_id = msgs[0].chat_id;
        let min_date = msgs.iter().map(|m| m.date).min().unwrap_or(0);
        // Apple ns since 2001
        let apple_min = (min_date - 978_307_200_000).max(0) * 1_000_000;
        let sql = format!(
            "SELECT {MSG_COLS} FROM message m JOIN chat_message_join c ON c.message_id = m.ROWID \
             WHERE c.chat_id = ?1 AND m.associated_message_type BETWEEN 2000 AND 3999 AND m.date >= ?2 ORDER BY m.date ASC"
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let mut rows = st.query(params![chat_id, apple_min])?;
        let mut by_guid: HashMap<String, Vec<Reaction>> = HashMap::new();
        while let Some(row) = rows.next()? {
            let m = DbMessage::from_row(row)?;
            let Some((_, target)) = m.clean_associated_guid() else { continue };
            let target = target.to_string();
            let Variant::Tapback(_, action, tb) = m.variant() else { continue };
            let (kind, emoji) = match tb {
                Tapback::Loved => ("love", "❤️".to_string()),
                Tapback::Liked => ("like", "👍".to_string()),
                Tapback::Disliked => ("dislike", "👎".to_string()),
                Tapback::Laughed => ("laugh", "😂".to_string()),
                Tapback::Emphasized => ("emphasize", "‼️".to_string()),
                Tapback::Questioned => ("question", "❓".to_string()),
                Tapback::Emoji(e) => ("emoji", e.unwrap_or("").to_string()),
                Tapback::Sticker => ("sticker", "🏷".to_string()),
            };
            let sender = if m.is_from_me { None } else { m.handle_id.and_then(|h| self.participant(h as i64, book)) };
            let key = |r: &Reaction| (r.kind.clone(), r.emoji.clone(), r.from_me, r.sender.as_ref().map(|s| s.handle_id));
            let list = by_guid.entry(target).or_default();
            let new = Reaction { kind: kind.into(), emoji, sender, from_me: m.is_from_me };
            let nk = key(&new);
            list.retain(|r| key(r) != nk);
            if action == TapbackAction::Added {
                list.push(new);
            }
        }
        for m in msgs.iter_mut() {
            if let Some(r) = by_guid.remove(&m.guid) {
                m.reactions = r;
            }
        }
        Ok(())
    }

    fn convert(&self, mut m: DbMessage, extra: Extra, book: &ContactBook) -> Result<Message> {
        if let Ok(b) = m.parse_body(&self.conn) {
            m.apply_body(b);
        }
        let sender = if m.is_from_me { None } else { m.handle_id.and_then(|h| self.participant(h as i64, book)) };
        let text_raw = m.text.clone();
        let mut parts: Vec<TextPart> = vec![];
        if let Some(t) = &text_raw {
            for comp in &m.components {
                if let BubbleComponent::Run(ranges) = comp {
                    for r in ranges {
                        if r.attachment.is_some() {
                            continue;
                        }
                        // Run boundaries carry formatting only; preserve their whitespace so
                        // adjacent runs do not accidentally concatenate words.
                        let s = clean_text_run(&slice(t, r.start, r.end));
                        if s.is_empty() {
                            continue;
                        }
                        let effects = r.effects.iter().filter_map(effect_name).collect();
                        parts.push(TextPart { text: s, effects });
                    }
                }
            }
        }
        let text = text_raw.map(|t| clean_text(&t)).filter(|t| !t.is_empty());
        if parts.is_empty() {
            if let Some(t) = &text {
                parts.push(TextPart { text: t.clone(), effects: vec![] });
            }
        }

        let attachments = DbAttachment::from_message(&self.conn, &m)
            .unwrap_or_default()
            .into_iter()
            .map(|a| {
                let path = a.filename.as_deref().map(expand_home);
                let available = path.as_ref().map(|p| p.exists()).unwrap_or(false);
                let name = a
                    .transfer_name
                    .clone()
                    .or_else(|| path.as_ref().and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string())))
                    .unwrap_or_else(|| "attachment".into());
                let mime = a.mime_type.clone().or_else(|| path.as_ref().and_then(|p| mime_guess::from_path(p).first().map(|m| m.to_string())))
                    .or_else(|| uti_to_mime(a.uti.as_deref()))
                    .unwrap_or_else(|| "application/octet-stream".into());
                Attachment { id: a.rowid as i64, guid: a.guid.clone(), name, mime, size: a.total_bytes, is_sticker: a.is_sticker, available, url: format!("/api/attachments/{}", a.rowid) }
            })
            .collect();

        let kind = if m.is_fully_unsent() || extra.date_retracted > 0 {
            MessageKind::Unsent
        } else if let Some(action) = m.group_action() {
            MessageKind::Announcement { text: self.group_action_text(&m, &action, book) }
        } else if m.is_shareplay() {
            MessageKind::Announcement { text: "SharePlay".into() }
        } else if let Variant::App(balloon) = m.variant() {
            let bundle_id = m.balloon_bundle_id.clone().unwrap_or_default();
            let (mut title, mut summary, mut url) = (None, None, None);
            if matches!(balloon, CustomBalloon::URL) {
                if let Some(payload) = m.payload_data(&self.conn) {
                    if let Ok(parsed) = parse_ns_keyed_archiver(&payload) {
                        if let Ok(u) = URLMessage::from_map(&parsed) {
                            title = u.title.map(str::to_string);
                            summary = u.summary.map(str::to_string);
                            url = u.get_url().map(str::to_string);
                        }
                    }
                }
                if url.is_none() {
                    url = text.clone();
                }
            } else {
                title = Some(match balloon {
                    CustomBalloon::ApplePay => "Apple Cash".into(),
                    CustomBalloon::Fitness => "Fitness".into(),
                    CustomBalloon::Slideshow => "Photos".into(),
                    CustomBalloon::CheckIn => "Check In".into(),
                    CustomBalloon::FindMy => "Find My".into(),
                    CustomBalloon::Polls => "Poll".into(),
                    CustomBalloon::Handwriting => "Handwriting".into(),
                    CustomBalloon::DigitalTouch => "Digital Touch".into(),
                    CustomBalloon::Business => "Business".into(),
                    CustomBalloon::Application(b) => b.rsplit('.').next().unwrap_or(b).to_string(),
                    CustomBalloon::URL => unreachable!(),
                });
            }
            MessageKind::App { bundle_id, title, summary, url }
        } else if m.item_type != 0 && text.is_none() && m.num_attachments == 0 {
            MessageKind::Announcement { text: "(system message)".into() }
        } else {
            MessageKind::Normal
        };

        let reply_to = if m.is_reply() {
            m.thread_originator_guid.as_ref().and_then(|g| self.reply_preview(g, book))
        } else {
            None
        };
        let expressive = match m.get_expressive() {
            Expressive::None => None,
            Expressive::Screen(s) => Some(match s {
                ScreenEffect::Confetti => "confetti",
                ScreenEffect::Echo => "echo",
                ScreenEffect::Fireworks => "fireworks",
                ScreenEffect::Balloons => "balloons",
                ScreenEffect::Heart => "heart",
                ScreenEffect::Lasers => "lasers",
                ScreenEffect::ShootingStar => "shooting star",
                ScreenEffect::Sparkles => "sparkles",
                ScreenEffect::Spotlight => "spotlight",
            }.to_string()),
            Expressive::Bubble(b) => Some(match b {
                BubbleEffect::Slam => "slam",
                BubbleEffect::Loud => "loud",
                BubbleEffect::Gentle => "gentle",
                BubbleEffect::InvisibleInk => "invisible ink",
            }.to_string()),
            Expressive::Unknown(s) => Some(s.to_string()),
        };
        let is_edited = m.edited_parts.as_ref().map(|e| e.parts.iter().any(|p| p.status == EditStatus::Edited)).unwrap_or(false);
        Ok(Message {
            id: m.rowid as i64,
            guid: m.guid.clone(),
            chat_id: m.chat_id.unwrap_or(0) as i64,
            sender,
            is_from_me: m.is_from_me,
            date: apple_ns_to_unix_ms(m.date),
            date_read: apple_ns_to_unix_ms(m.date_read),
            date_delivered: apple_ns_to_unix_ms(m.date_delivered),
            is_read: m.is_read,
            is_delivered: extra.is_delivered,
            is_sent: extra.is_sent,
            error: extra.error,
            service: m.service.clone().unwrap_or_else(|| "iMessage".into()),
            subject: m.subject.clone().filter(|s| !s.is_empty()),
            text,
            parts,
            attachments,
            reactions: vec![],
            kind,
            reply_to,
            thread_originator_guid: m.thread_originator_guid.clone(),
            is_edited,
            expressive,
            is_audio: extra.is_audio,
        })
    }

    fn reply_preview(&self, guid: &str, book: &ContactBook) -> Option<ReplyRef> {
        let mut st = self.conn.prepare_cached("SELECT text, attributedBody, is_from_me, handle_id, cache_has_attachments FROM message WHERE guid = ?1").ok()?;
        let r = st
            .query_row([guid], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<Vec<u8>>>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            })
            .ok()?;
        let (text, body, from_me, handle, has_att) = r;
        let mut preview = text.filter(|t| !t.trim().is_empty());
        if preview.is_none() {
            if let Some(b) = body {
                preview = imessage_database::util::streamtyped::parse(b).ok();
            }
        }
        let preview = preview.map(|t| t.replace('\u{FFFC}', "").trim().to_string()).filter(|t| !t.is_empty()).or(if has_att { Some("📎 Attachment".into()) } else { None });
        let sender_name = if from_me { Some("You".into()) } else { handle.and_then(|h| self.participant(h, book)).map(|p| p.display().to_string()) };
        Some(ReplyRef { guid: guid.to_string(), preview, sender_name })
    }

    fn group_action_text(&self, m: &DbMessage, action: &GroupAction, book: &ContactBook) -> String {
        let who = if m.is_from_me { "You".to_string() } else { m.handle_id.and_then(|h| self.participant(h as i64, book)).map(|p| p.display().to_string()).unwrap_or_else(|| "Someone".into()) };
        let other = |h: i32| self.participant(h as i64, book).map(|p| p.display().to_string()).unwrap_or_else(|| "someone".into());
        match action {
            GroupAction::ParticipantAdded(h) => format!("{who} added {}", other(*h)),
            GroupAction::ParticipantRemoved(h) => format!("{who} removed {}", other(*h)),
            GroupAction::NameChange(n) => format!("{who} named the conversation \"{n}\""),
            GroupAction::ParticipantLeft => format!("{who} left the conversation"),
            GroupAction::GroupIconChanged => format!("{who} changed the group photo"),
            GroupAction::GroupIconRemoved => format!("{who} removed the group photo"),
            GroupAction::ChatBackgroundChanged => format!("{who} changed the background"),
            GroupAction::ChatBackgroundRemoved => format!("{who} removed the background"),
            GroupAction::PhoneNumberChanged(_) => format!("{who} changed their number"),
        }
    }
}

struct Extra {
    is_delivered: bool,
    is_sent: bool,
    error: i64,
    is_audio: bool,
    date_retracted: i64,
}

impl Extra {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Extra {
            is_delivered: row.get::<_, Option<bool>>("is_delivered")?.unwrap_or(false),
            is_sent: row.get::<_, Option<bool>>("is_sent")?.unwrap_or(false),
            error: row.get::<_, Option<i64>>("error")?.unwrap_or(0),
            is_audio: row.get::<_, Option<bool>>("is_audio_message")?.unwrap_or(false),
            date_retracted: row.get::<_, Option<i64>>("date_retracted")?.unwrap_or(0),
        })
    }
}

fn slice(t: &str, start: usize, end: usize) -> String {
    let end = end.min(t.len());
    let start = start.min(end);
    match t.get(start..end) {
        Some(s) => s.to_string(),
        None => {
            // Fall back to char boundaries.
            let mut s = start;
            while s > 0 && !t.is_char_boundary(s) { s -= 1; }
            let mut e = end;
            while e < t.len() && !t.is_char_boundary(e) { e += 1; }
            t.get(s..e).unwrap_or("").to_string()
        }
    }
}

fn effect_name(e: &TextEffect) -> Option<String> {
    Some(match e {
        TextEffect::Default => return None,
        TextEffect::Mention(m) => format!("mention:{m}"),
        TextEffect::Link(u) => format!("link:{u}"),
        TextEffect::OTP => "otp".into(),
        TextEffect::Styles(styles) => {
            return Some(
                styles
                    .iter()
                    .map(|s| match s {
                        Style::Bold => "bold",
                        Style::Italic => "italic",
                        Style::Strikethrough => "strikethrough",
                        Style::Underline => "underline",
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            )
        }
        TextEffect::Animated(a) => format!(
            "animated:{}",
            match a {
                Animation::Big => "big",
                Animation::Small => "small",
                Animation::Shake => "shake",
                Animation::Nod => "nod",
                Animation::Explode => "explode",
                Animation::Ripple => "ripple",
                Animation::Bloom => "bloom",
                Animation::Jitter => "jitter",
                Animation::Unknown(_) => "unknown",
            }
        ),
        _ => return None,
    })
}

/// Strip object-replacement placeholders and Apple's internal breadcrumb tokens.
fn clean_text(t: &str) -> String {
    clean_text_run(t)
        .trim_matches(|c: char| c == '\n' || c == ' ')
        .to_string()
}

/// Remove non-message markers while retaining whitespace inside a formatted run.
fn clean_text_run(t: &str) -> String {
    let t = t.replace('\u{FFFC}', "").replace('\u{FFFD}', "");
    t.replace("$(kIMTranscriptPluginBreadcrumbTextReceiverIdentifier)", "")
        .replace("$(kIMTranscriptPluginBreadcrumbTextSenderIdentifier)", "")
}

#[cfg(test)]
mod tests {
    use super::{clean_text, clean_text_run, Db};
    use rusqlite::Connection;

    fn fixture(dir: &std::path::Path) -> std::path::PathBuf {
        // Minimal slice of the chat.db schema: two incoming unread messages in chat 1.
        let path = dir.join("chat.db");
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, guid TEXT, chat_identifier TEXT, service_name TEXT, display_name TEXT, style INTEGER);
             CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER, message_date INTEGER);
             CREATE TABLE message (ROWID INTEGER PRIMARY KEY, is_from_me INTEGER, is_read INTEGER, item_type INTEGER, associated_message_type INTEGER);
             INSERT INTO chat VALUES (1, 'g1', 'c1', 'iMessage', NULL, 45);
             INSERT INTO message VALUES (10, 0, 0, 0, 0), (11, 0, 0, 0, 0);
             INSERT INTO chat_message_join VALUES (1, 10, 100), (1, 11, 200);",
        )
        .unwrap();
        path
    }

    fn unread(db: &mut Db, chat: i64) -> i64 {
        let seen = db.seen.get(&chat).copied().unwrap_or(0);
        db.conn
            .query_row(
                "SELECT COUNT(*) FROM message m JOIN chat_message_join j ON j.message_id = m.ROWID WHERE j.chat_id = ?1 AND m.ROWID > ?2 AND m.is_from_me = 0 AND m.is_read = 0",
                rusqlite::params![chat, seen],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn seen_marks_reduce_unread_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = fixture(dir.path());
        let seen_path = dir.path().join("seen.json");
        let mut db = Db::open(&db_path).unwrap().with_seen_file(seen_path.clone());
        assert_eq!(unread(&mut db, 1), 2);
        assert_eq!(db.mark_seen(1, Some(10)).unwrap(), 10);
        assert_eq!(unread(&mut db, 1), 1);
        // Marking an older message never moves the mark backwards.
        assert_eq!(db.mark_seen(1, Some(5)).unwrap(), 10);
        // No id means "everything currently in the chat".
        assert_eq!(db.mark_seen(1, None).unwrap(), 11);
        assert_eq!(unread(&mut db, 1), 0);
        drop(db);
        let mut again = Db::open(&db_path).unwrap().with_seen_file(seen_path);
        assert_eq!(again.seen.get(&1), Some(&11));
        assert_eq!(unread(&mut again, 1), 0);
    }

    #[test]
    fn formatted_runs_preserve_spaces_and_line_breaks() {
        let runs = ["Hello", " ", "world", "\n", "again"];
        let text = runs.iter().map(|run| clean_text_run(run)).collect::<String>();

        assert_eq!(text, "Hello world\nagain");
    }

    #[test]
    fn formatted_runs_remove_internal_tokens_without_trimming_whitespace() {
        let token = "$(kIMTranscriptPluginBreadcrumbTextSenderIdentifier)";

        assert_eq!(clean_text_run("  "), "  ");
        assert_eq!(clean_text_run("\n"), "\n");
        assert_eq!(clean_text_run(&format!(" {token}\n")), " \n");
        assert_eq!(clean_text_run("a\u{FFFC}b\u{FFFD}"), "ab");
    }

    #[test]
    fn whole_message_cleaning_still_trims_outer_whitespace() {
        assert_eq!(clean_text("\n Hello world \n"), "Hello world");
    }
}

pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(format!("{home}{rest}"))
    } else {
        PathBuf::from(p)
    }
}

fn uti_to_mime(uti: Option<&str>) -> Option<String> {
    Some(match uti? {
        "public.heic" | "public.heif" => "image/heic",
        "public.jpeg" => "image/jpeg",
        "public.png" => "image/png",
        "com.compuserve.gif" => "image/gif",
        "public.mpeg-4" | "com.apple.quicktime-movie" => "video/mp4",
        "public.mp3" => "audio/mpeg",
        "com.apple.m4a-audio" | "public.mpeg-4-audio" => "audio/mp4",
        "com.apple.coreaudio-format" => "audio/x-caf",
        "com.adobe.pdf" => "application/pdf",
        "public.vcard" => "text/vcard",
        _ => return None,
    }
    .to_string())
}
