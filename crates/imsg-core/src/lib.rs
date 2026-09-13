//! Shared wire types between the imsg server and its clients (TUI, web).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Participant {
    pub handle_id: i64,
    /// Raw address: phone number in E.164 or email.
    pub address: String,
    /// Resolved contact name, if any.
    pub name: Option<String>,
    pub contact_id: Option<i64>,
}

impl Participant {
    pub fn display(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.address)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: i64,
    pub guid: String,
    pub identifier: String,
    pub service: String,
    pub display_name: Option<String>,
    pub is_group: bool,
    pub participants: Vec<Participant>,
    /// Unix ms of the most recent message.
    pub last_date: i64,
    pub last_preview: Option<String>,
    pub last_from_me: bool,
    pub unread: i64,
}

impl Chat {
    pub fn title(&self) -> String {
        if let Some(n) = &self.display_name {
            if !n.is_empty() {
                return n.clone();
            }
        }
        if self.participants.is_empty() {
            return self.identifier.clone();
        }
        self.participants
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: i64,
    pub guid: Option<String>,
    pub name: String,
    pub mime: String,
    pub size: i64,
    pub is_sticker: bool,
    /// True when the file exists on the server's disk.
    pub available: bool,
    /// Relative URL, e.g. `/api/attachments/123`.
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reaction {
    /// love | like | dislike | laugh | emphasize | question | emoji | sticker
    pub kind: String,
    pub emoji: String,
    pub sender: Option<Participant>,
    pub from_me: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TextPart {
    pub text: String,
    /// bold, italic, strikethrough, underline, link:<url>, mention, otp, animated:<name>
    pub effects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageKind {
    Normal,
    /// System line such as "Alice named the group X".
    Announcement { text: String },
    /// App / rich link balloon.
    App {
        bundle_id: String,
        title: Option<String>,
        summary: Option<String>,
        url: Option<String>,
    },
    Unsent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub guid: String,
    pub chat_id: i64,
    pub sender: Option<Participant>,
    pub is_from_me: bool,
    pub date: i64,
    pub date_read: i64,
    pub date_delivered: i64,
    pub is_read: bool,
    pub is_delivered: bool,
    pub is_sent: bool,
    pub error: i64,
    pub service: String,
    pub subject: Option<String>,
    pub text: Option<String>,
    pub parts: Vec<TextPart>,
    pub attachments: Vec<Attachment>,
    pub reactions: Vec<Reaction>,
    pub kind: MessageKind,
    pub reply_to: Option<ReplyRef>,
    pub thread_originator_guid: Option<String>,
    pub is_edited: bool,
    pub expressive: Option<String>,
    pub is_audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyRef {
    pub guid: String,
    pub preview: Option<String>,
    pub sender_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub id: i64,
    pub name: String,
    pub first: Option<String>,
    pub last: Option<String>,
    pub organization: Option<String>,
    pub phones: Vec<String>,
    pub emails: Vec<String>,
    pub has_photo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SendRequest {
    /// Existing chat GUID (preferred).
    pub chat_guid: Option<String>,
    /// Or addresses for a new conversation.
    #[serde(default)]
    pub to: Vec<String>,
    pub text: Option<String>,
    /// Upload ids returned from POST /api/upload, sent as file attachments.
    #[serde(default)]
    pub uploads: Vec<String>,
    /// Absolute file paths on the server machine (local TUI convenience).
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendResponse {
    pub ok: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadResponse {
    pub id: String,
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesPage {
    pub messages: Vec<Message>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub me: Vec<String>,
    pub host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WsEvent {
    Hello { info: ServerInfo },
    /// New or changed messages. Clients should merge by id.
    Messages { messages: Vec<Message> },
    /// Chat list changed (new chat, reorder, unread count).
    Chats { chats: Vec<Chat> },
    Error { message: String },
}

/// Convert an Apple "nanoseconds since 2001-01-01" timestamp to unix ms.
pub fn apple_ns_to_unix_ms(ts: i64) -> i64 {
    if ts == 0 {
        return 0;
    }
    const OFFSET_S: i64 = 978_307_200;
    let ts = if ts > 10_000_000_000 { ts / 1_000_000 } else { ts * 1000 };
    ts + OFFSET_S * 1000
}

/// Normalize a phone number or email for matching: emails lowercased, phones reduced to digits.
pub fn normalize_address(s: &str) -> String {
    let s = s.trim();
    if s.contains('@') {
        return s.to_lowercase();
    }
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 11 && digits.starts_with('1') {
        digits[1..].to_string()
    } else {
        digits
    }
}

pub fn tapback_emoji(kind: &str) -> &'static str {
    match kind {
        "love" => "❤️",
        "like" => "👍",
        "dislike" => "👎",
        "laugh" => "😂",
        "emphasize" => "‼️",
        "question" => "❓",
        _ => "",
    }
}
