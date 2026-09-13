use crate::app::{human_size, App, Focus, Mode};
use chrono::{DateTime, Datelike, Local, TimeZone};
use imsg_core::*;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

const BLUE: Color = Color::Rgb(0, 122, 255);
const GREEN: Color = Color::Rgb(52, 199, 89);
const GRAY: Color = Color::Rgb(58, 58, 60);
const DIM: Color = Color::Rgb(142, 142, 147);
const ACCENT: Color = Color::Rgb(255, 214, 10);

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_width(area.width)), Constraint::Min(30)])
        .split(area);
    draw_sidebar(f, app, cols[0]);

    let compose_h = (app.compose.lines().len() as u16).clamp(1, 6) + 2;
    let chips_h = if app.pending_files.is_empty() { 0 } else { 1 };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(5), Constraint::Length(chips_h), Constraint::Length(compose_h), Constraint::Length(1)])
        .split(cols[1]);
    draw_header(f, app, rows[0]);
    draw_messages(f, app, rows[1]);
    if chips_h > 0 {
        draw_chips(f, app, rows[2]);
    }
    draw_compose(f, app, rows[3]);
    draw_status(f, app, rows[4]);

    match app.mode {
        Mode::Emoji => draw_emoji_picker(f, app, area),
        Mode::Contacts => draw_contact_picker(f, app, area),
        Mode::Attach => draw_attach(f, app, area),
        Mode::Search => draw_search(f, app, area),
        Mode::Help => draw_help(f, area),
        _ => {}
    }
}

fn sidebar_width(total: u16) -> u16 {
    (total * 30 / 100).clamp(24, 44)
}

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Chats && app.mode == Mode::Normal;
    let title = if app.mode == Mode::ChatFilter || !app.chat_filter.is_empty() {
        format!(" / {}▏", app.chat_filter)
    } else {
        " Messages ".to_string()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused || app.mode == Mode::ChatFilter { Style::default().fg(BLUE) } else { Style::default().fg(DIM) })
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = inner.width as usize;
    let idxs = app.filtered_chats();
    let mut items = Vec::with_capacity(idxs.len() + 1);
    if let Some(nc) = &app.new_chat {
        items.push(ListItem::new(Text::from(vec![
            Line::from(Span::styled(format!("✎ New: {}", nc.name), Style::default().fg(ACCENT).bold())),
            Line::from(Span::styled(truncate(&nc.address, w.saturating_sub(2)), Style::default().fg(DIM))),
        ])));
    }
    for i in &idxs {
        let c = &app.chats[*i];
        let title = c.title();
        let time = short_time(c.last_date);
        let tw = time.width();
        let name_w = w.saturating_sub(tw + 4);
        let dot = if c.unread > 0 { "●" } else { " " };
        let name_style = if c.unread > 0 { Style::default().bold() } else { Style::default() };
        let mut l1 = vec![Span::styled(dot, Style::default().fg(BLUE)), Span::raw(" "), Span::styled(pad(&truncate(&title, name_w), name_w), name_style), Span::raw(" "), Span::styled(time, Style::default().fg(DIM))];
        if c.is_group {
            l1.insert(2, Span::styled("👥", Style::default()));
            l1[3] = Span::styled(pad(&truncate(&title, name_w.saturating_sub(2)), name_w.saturating_sub(2)), name_style);
        }
        let mut prev = c.last_preview.clone().unwrap_or_default().replace('\n', " ");
        if c.last_from_me && !prev.is_empty() {
            prev = format!("You: {prev}");
        }
        let l2 = Line::from(Span::styled(format!("  {}", truncate(&prev, w.saturating_sub(2))), Style::default().fg(DIM)));
        items.push(ListItem::new(Text::from(vec![Line::from(l1), l2])));
    }
    let mut state = ListState::default();
    let sel = if app.new_chat.is_some() { Some(0) } else if idxs.is_empty() { None } else { Some(app.chat_sel.min(idxs.len() - 1)) };
    state.select(sel);
    let list = List::new(items).highlight_style(Style::default().bg(Color::Rgb(44, 44, 46)));
    f.render_stateful_widget(list, inner, &mut state);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let (title, sub) = if let Some(nc) = &app.new_chat {
        (format!("New message to {}", nc.name), nc.address.clone())
    } else if let Some(c) = app.current_chat() {
        let members = if c.is_group {
            c.participants.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        } else {
            c.participants.first().map(|p| p.address.clone()).unwrap_or_default()
        };
        (c.title(), format!("{} · {}", c.service, members))
    } else {
        ("imsg".to_string(), "select a conversation".to_string())
    };
    let text = Text::from(vec![
        Line::from(Span::styled(title, Style::default().bold())),
        Line::from(Span::styled(truncate(&sub, area.width as usize), Style::default().fg(DIM))),
    ]);
    f.render_widget(Paragraph::new(text), area);
}

struct Rendered {
    lines: Vec<Line<'static>>,
    ranges: Vec<(usize, usize)>,
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Messages && app.mode == Mode::Normal;
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(if focused { Style::default().fg(BLUE) } else { Style::default().fg(DIM) });
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let height = inner.height as usize;
    let is_group = app.current_chat().map(|c| c.is_group).unwrap_or(false);
    let sms = app.current_chat().map(|c| c.service != "iMessage").unwrap_or(false);
    let msgs: Vec<Message> = app.current_messages().to_vec();
    let loading = app.current_chat_id().map(|id| app.loading.contains(&id)).unwrap_or(false);
    let has_more = app.current_chat_id().and_then(|id| app.has_more.get(&id).copied()).unwrap_or(false);
    let r = render_messages(&msgs, width, is_group, sms, app.msg_sel, loading, has_more);
    app.msg_total_lines = r.lines.len();
    app.msg_view_height = height;
    app.msg_line_ranges = r.ranges;
    let total = r.lines.len();
    let max_scroll = total.saturating_sub(height);
    if app.scroll > max_scroll {
        app.scroll = max_scroll;
    }
    let offset = total.saturating_sub(height + app.scroll);
    let text = Text::from(r.lines);
    let p = Paragraph::new(text).scroll((offset as u16, 0));
    f.render_widget(p, inner);
}

fn render_messages(msgs: &[Message], width: usize, is_group: bool, sms: bool, sel: Option<usize>, loading: bool, has_more: bool) -> Rendered {
    let mut lines: Vec<Line<'static>> = vec![];
    let mut ranges = vec![];
    if loading {
        lines.push(Line::from(Span::styled("loading…", Style::default().fg(DIM))).alignment(Alignment::Center));
    } else if has_more {
        lines.push(Line::from(Span::styled("↑ scroll for earlier messages", Style::default().fg(DIM))).alignment(Alignment::Center));
    }
    let max_bubble = (width * 65 / 100).clamp(12, width.saturating_sub(4).max(12));
    let mut last_date: i64 = 0;
    let mut last_from_me_idx: Option<usize> = None;
    for (i, m) in msgs.iter().enumerate() {
        if m.is_from_me {
            last_from_me_idx = Some(i);
        }
    }
    for (i, m) in msgs.iter().enumerate() {
        let start = lines.len();
        if m.date - last_date > 60 * 60 * 1000 {
            lines.push(Line::from(Span::styled(format!(" {} ", full_time(m.date)), Style::default().fg(DIM))).alignment(Alignment::Center));
        }
        last_date = m.date;
        let selected = sel == Some(i);
        match &m.kind {
            MessageKind::Announcement { text } => {
                lines.push(Line::from(Span::styled(text.clone(), Style::default().fg(DIM).italic())).alignment(Alignment::Center));
            }
            _ => {
                let color = if m.is_from_me { if sms || m.service != "iMessage" { GREEN } else { BLUE } } else { GRAY };
                let fg = Color::White;
                // Sender name in groups
                if is_group && !m.is_from_me {
                    let name = m.sender.as_ref().map(|s| s.display().to_string()).unwrap_or_else(|| "Unknown".into());
                    lines.push(Line::from(Span::styled(format!("  {name}"), Style::default().fg(DIM))));
                }
                // Reply quote
                if let Some(r) = &m.reply_to {
                    let q = format!("↩ {}: {}", r.sender_name.clone().unwrap_or_default(), r.preview.clone().unwrap_or_default().replace('\n', " "));
                    let q = truncate(&q, max_bubble + 2);
                    let pad_l = if m.is_from_me { width.saturating_sub(q.width() + 1) } else { 2 };
                    lines.push(Line::from(vec![Span::raw(" ".repeat(pad_l)), Span::styled(q, Style::default().fg(DIM).italic())]));
                }
                // Body segments
                let mut segs: Vec<(String, Style)> = vec![];
                let base = Style::default().fg(fg).bg(color);
                if let Some(s) = &m.subject {
                    segs.push((format!("{s}\n"), base.bold()));
                }
                match &m.kind {
                    MessageKind::Unsent => segs.push(("(message unsent)".into(), base.italic())),
                    MessageKind::App { title, summary, url, bundle_id } => {
                        if let Some(t) = title { segs.push((format!("{t}\n"), base.bold())); }
                        if let Some(s) = summary { segs.push((format!("{}\n", truncate(s, 200)), base)); }
                        if let Some(u) = url { segs.push((u.clone(), base.underlined())); } else if title.is_none() { segs.push((format!("[{bundle_id}]"), base.italic())); }
                    }
                    _ => {
                        for p in &m.parts {
                            let mut st = base;
                            for e in &p.effects {
                                for tok in e.split(',') {
                                    match tok {
                                        "bold" => st = st.bold(),
                                        "italic" => st = st.italic(),
                                        "strikethrough" => st = st.crossed_out(),
                                        "underline" => st = st.underlined(),
                                        "otp" => st = st.bold(),
                                        t if t.starts_with("link:") => st = st.underlined(),
                                        t if t.starts_with("mention:") => st = st.bold(),
                                        t if t.starts_with("animated:") => st = st.italic(),
                                        _ => {}
                                    }
                                }
                            }
                            segs.push((p.text.clone(), st));
                        }
                    }
                }
                for a in &m.attachments {
                    if matches!(m.kind, MessageKind::App { .. }) && a.name.ends_with(".pluginPayloadAttachment") {
                        continue;
                    }
                    let icon = att_icon(&a.mime, a.is_sticker);
                    let recent_mine = m.is_from_me && (chrono::Utc::now().timestamp_millis() - m.date) < 600_000;
                    let label = if a.available { format!("{icon} {} ({})", a.name, human_size(a.size)) } else if recent_mine { format!("{icon} {} (sending…)", a.name) } else { format!("{icon} {} (not on this Mac)", a.name) };
                    if !segs.is_empty() && !segs.last().map(|s| s.0.ends_with('\n')).unwrap_or(true) {
                        segs.push(("\n".into(), base));
                    }
                    segs.push((label, base.underlined()));
                    segs.push(("\n".into(), base));
                }
                if m.is_audio && m.attachments.is_empty() {
                    segs.push(("🎤 Audio message".into(), base.italic()));
                }
                if let Some(e) = &m.expressive {
                    segs.push((format!("\n✨ sent with {e}"), base.italic()));
                }
                if m.is_edited {
                    segs.push(("\nEdited".into(), base.italic().fg(Color::Rgb(220, 220, 220))));
                }
                if segs.is_empty() {
                    let recent_mine = m.is_from_me && (chrono::Utc::now().timestamp_millis() - m.date) < 600_000;
                    segs.push(((if recent_mine { "Sending…" } else { "(no content)" }).into(), base.italic()));
                }
                let wrapped = wrap_segments(&segs, max_bubble);
                let bw = wrapped.iter().map(|l| l.iter().map(|s| s.content.width()).sum::<usize>()).max().unwrap_or(1) + 2;
                let pad_l = if m.is_from_me { width.saturating_sub(bw + 1) } else { 2 };
                let marker = if selected { Span::styled("▶", Style::default().fg(ACCENT)) } else { Span::raw(" ") };
                // Top edge
                let mut top = vec![Span::raw(" ".repeat(pad_l.saturating_sub(1))), marker.clone(), Span::styled("▄".repeat(bw), Style::default().fg(color))];
                if !m.is_from_me { top.insert(0, Span::raw("")); }
                lines.push(Line::from(top));
                let n = wrapped.len();
                for (li, l) in wrapped.into_iter().enumerate() {
                    let used: usize = l.iter().map(|s| s.content.width()).sum();
                    let mut spans = vec![Span::raw(" ".repeat(pad_l)), Span::styled(" ", base)];
                    spans.extend(l);
                    spans.push(Span::styled(" ".repeat(bw.saturating_sub(used + 1)), base));
                    if li + 1 == n {
                        // time next to the last row
                        let t = short_clock(m.date);
                        if m.is_from_me {
                            let tl = Span::styled(t, Style::default().fg(DIM));
                            let mut v = vec![Span::raw(" ".repeat(pad_l.saturating_sub(tl.content.width() + 1))), tl, Span::raw(" ")];
                            v.extend(spans.into_iter().skip(1));
                            spans = v;
                        } else {
                            spans.push(Span::raw(" "));
                            spans.push(Span::styled(t, Style::default().fg(DIM)));
                        }
                    }
                    lines.push(Line::from(spans));
                }
                lines.push(Line::from(vec![Span::raw(" ".repeat(pad_l)), Span::styled("▀".repeat(bw), Style::default().fg(color))]));
                // Reactions
                if !m.reactions.is_empty() {
                    let r: Vec<String> = m.reactions.iter().map(|r| format!("{} {}", r.emoji, if r.from_me { "You".to_string() } else { r.sender.as_ref().map(|s| first_name(s.display())).unwrap_or_default() })).collect();
                    let s = r.join("  ");
                    let pl = if m.is_from_me { width.saturating_sub(s.width() + 1) } else { 3 };
                    lines.push(Line::from(vec![Span::raw(" ".repeat(pl)), Span::styled(s, Style::default().fg(Color::Rgb(200, 200, 205)))]));
                }
                // Delivery status under the latest own message
                if last_from_me_idx == Some(i) && m.is_from_me {
                    let s = if m.error != 0 { "Not delivered".to_string() } else if m.date_read > 0 { format!("Read {}", short_clock(m.date_read)) } else if m.is_delivered || m.date_delivered > 0 { "Delivered".into() } else if m.is_sent { "Sent".into() } else { String::new() };
                    if !s.is_empty() {
                        let st = if m.error != 0 { Style::default().fg(Color::Red) } else { Style::default().fg(DIM) };
                        lines.push(Line::from(vec![Span::raw(" ".repeat(width.saturating_sub(s.width() + 1))), Span::styled(s, st)]));
                    }
                }
            }
        }
        lines.push(Line::from(""));
        ranges.push((start, lines.len().saturating_sub(1)));
    }
    if msgs.is_empty() && !loading {
        lines.push(Line::from(Span::styled("No messages yet", Style::default().fg(DIM))).alignment(Alignment::Center));
    }
    Rendered { lines, ranges }
}

/// Style-preserving greedy word wrap. Returns rows of spans.
fn wrap_segments(segs: &[(String, Style)], max_w: usize) -> Vec<Vec<Span<'static>>> {
    let max_w = max_w.max(4);
    let mut rows: Vec<Vec<Span<'static>>> = vec![vec![]];
    let mut cur_w = 0usize;
    let push = |rows: &mut Vec<Vec<Span<'static>>>, cur_w: &mut usize, s: &str, st: Style| {
        if s.is_empty() { return; }
        rows.last_mut().unwrap().push(Span::styled(s.to_string(), st));
        *cur_w += s.width();
    };
    for (text, st) in segs {
        let mut first_line = true;
        for line in text.split('\n') {
            if !first_line {
                rows.push(vec![]);
                cur_w = 0;
            }
            first_line = false;
            // tokens: words and spaces
            let mut tok = String::new();
            let mut tokens: Vec<String> = vec![];
            for ch in line.chars() {
                if ch == ' ' {
                    if !tok.is_empty() { tokens.push(std::mem::take(&mut tok)); }
                    tokens.push(" ".into());
                } else {
                    tok.push(ch);
                }
            }
            if !tok.is_empty() { tokens.push(tok); }
            for t in tokens {
                let tw = t.width();
                if t == " " {
                    if cur_w == 0 { continue; }
                    if cur_w + 1 > max_w { rows.push(vec![]); cur_w = 0; continue; }
                    push(&mut rows, &mut cur_w, " ", *st);
                    continue;
                }
                if cur_w + tw <= max_w {
                    push(&mut rows, &mut cur_w, &t, *st);
                } else if tw <= max_w {
                    // trim trailing space of previous row
                    trim_row(rows.last_mut().unwrap());
                    rows.push(vec![]);
                    cur_w = 0;
                    push(&mut rows, &mut cur_w, &t, *st);
                } else {
                    // long token: split by chars
                    let mut piece = String::new();
                    for ch in t.chars() {
                        let cw = UnicodeWidthStr::width(ch.to_string().as_str());
                        if cur_w + piece.width() + cw > max_w {
                            push(&mut rows, &mut cur_w, &piece, *st);
                            piece.clear();
                            rows.push(vec![]);
                            cur_w = 0;
                        }
                        piece.push(ch);
                    }
                    push(&mut rows, &mut cur_w, &piece, *st);
                }
            }
        }
    }
    for r in rows.iter_mut() { trim_row(r); }
    // Remove trailing empty rows but keep at least one.
    while rows.len() > 1 && rows.last().map(|r| r.is_empty()).unwrap_or(false) { rows.pop(); }
    rows
}

fn trim_row(row: &mut Vec<Span<'static>>) {
    while let Some(last) = row.last_mut() {
        let t = last.content.trim_end().to_string();
        if t.is_empty() { row.pop(); } else { last.content = t.into(); break; }
    }
}

fn draw_chips(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::styled(" 📎 ", Style::default().fg(DIM))];
    for u in &app.pending_files {
        spans.push(Span::styled(format!(" {} ", u.name), Style::default().bg(Color::Rgb(44, 44, 46)).fg(Color::White)));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled("(Backspace on empty input removes)", Style::default().fg(DIM)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_compose(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Compose && app.mode == Mode::Normal;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused { Style::default().fg(BLUE) } else { Style::default().fg(DIM) });
    app.compose.set_block(block);
    f.render_widget(&app.compose, area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let conn = if app.connected { Span::styled("● live", Style::default().fg(GREEN)) } else { Span::styled("○ offline", Style::default().fg(Color::Red)) };
    let host = app.info.as_ref().map(|i| format!(" {} ", i.host)).unwrap_or_default();
    let keys = match (app.focus, app.mode) {
        (_, Mode::Normal) => match app.focus {
            Focus::Chats => "j/k move · Enter compose · / filter · Ctrl+N new · Ctrl+S search · ? help",
            Focus::Messages => "j/k select · o open · s save · y copy · PgUp/PgDn scroll · Esc back",
            Focus::Compose => "Enter send · Alt+Enter newline · :smile: emoji · Ctrl+E picker · Ctrl+F file · Esc",
        },
        _ => "Esc cancel",
    };
    let status = truncate(&app.status, area.width as usize / 2);
    let line = Line::from(vec![conn, Span::styled(host, Style::default().fg(DIM)), Span::styled(status, Style::default().fg(ACCENT)), Span::raw("  "), Span::styled(keys, Style::default().fg(DIM))]);
    f.render_widget(Paragraph::new(line), area);
}

fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn draw_emoji_picker(f: &mut Frame, app: &App, area: Rect) {
    let r = popup(area, 50, 18);
    f.render_widget(Clear, r);
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(BLUE)).title(format!(" Emoji: {}▏", app.emoji_query));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let matches = app.emoji_matches();
    let h = inner.height as usize;
    let start = app.emoji_sel.saturating_sub(h.saturating_sub(1));
    let items: Vec<ListItem> = matches.iter().skip(start).take(h).map(|e| {
        let codes: Vec<String> = e.shortcodes().take(2).map(|s| format!(":{s}:")).collect();
        ListItem::new(Line::from(vec![Span::raw(format!("{}  ", e.as_str())), Span::raw(e.name().to_string()), Span::styled(format!("  {}", codes.join(" ")), Style::default().fg(DIM))]))
    }).collect();
    let mut st = ListState::default();
    st.select(Some(app.emoji_sel - start));
    f.render_stateful_widget(List::new(items).highlight_style(Style::default().bg(Color::Rgb(44, 44, 46))), inner, &mut st);
}

fn draw_contact_picker(f: &mut Frame, app: &App, area: Rect) {
    let r = popup(area, 60, 20);
    f.render_widget(Clear, r);
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(BLUE)).title(format!(" To: {}▏", app.picker_query));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let matches = app.contact_matches();
    if matches.is_empty() {
        let msg = if app.contacts.is_empty() { "loading contacts…" } else { "no matches — type a phone number or email" };
        f.render_widget(Paragraph::new(Span::styled(msg, Style::default().fg(DIM))), inner);
        return;
    }
    let h = inner.height as usize;
    let start = app.picker_sel.saturating_sub(h.saturating_sub(1));
    let items: Vec<ListItem> = matches.iter().skip(start).take(h).map(|(n, a, _)| ListItem::new(Line::from(vec![Span::raw(n.clone()), Span::styled(format!("  {a}"), Style::default().fg(DIM))]))).collect();
    let mut st = ListState::default();
    st.select(Some(app.picker_sel - start));
    f.render_stateful_widget(List::new(items).highlight_style(Style::default().bg(Color::Rgb(44, 44, 46))), inner, &mut st);
}

fn draw_attach(f: &mut Frame, app: &App, area: Rect) {
    let r = popup(area, 70, 3);
    f.render_widget(Clear, r);
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(BLUE)).title(" Attach file (path, Tab completes) ");
    let inner = block.inner(r);
    f.render_widget(block, r);
    f.render_widget(Paragraph::new(format!("{}▏", app.attach_input)), inner);
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let r = popup(area, 80, 22);
    f.render_widget(Clear, r);
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(BLUE)).title(format!(" Search messages: {}▏  (Enter to search / jump) ", app.search_query));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let h = inner.height as usize;
    let start = app.search_sel.saturating_sub(h.saturating_sub(1));
    let items: Vec<ListItem> = app.search_results.iter().skip(start).take(h).map(|m| {
        let who = if m.is_from_me { "You".to_string() } else { m.sender.as_ref().map(|s| s.display().to_string()).unwrap_or_default() };
        let chat = app.chats.iter().find(|c| c.id == m.chat_id).map(|c| c.title()).unwrap_or_default();
        ListItem::new(Line::from(vec![
            Span::styled(short_time(m.date), Style::default().fg(DIM)),
            Span::raw(" "),
            Span::styled(truncate(&chat, 18), Style::default().fg(BLUE)),
            Span::styled(format!(" {who}: "), Style::default().fg(DIM)),
            Span::raw(truncate(&m.text.clone().unwrap_or_default().replace('\n', " "), inner.width as usize)),
        ]))
    }).collect();
    let mut st = ListState::default();
    if !app.search_results.is_empty() { st.select(Some(app.search_sel - start)); }
    f.render_stateful_widget(List::new(items).highlight_style(Style::default().bg(Color::Rgb(44, 44, 46))), inner, &mut st);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let r = popup(area, 76, 24);
    f.render_widget(Clear, r);
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(BLUE)).title(" imsg keys (any key closes) ");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let text = "\
Global
  Tab / Shift+Tab   cycle chats → messages → compose
  Ctrl+N            new message (contact picker)
  Ctrl+E            emoji picker   (or type :shortcode: inline)
  Ctrl+F            attach a file by path
  Ctrl+S            search all messages
  Ctrl+R            refresh            Ctrl+Q / Ctrl+C quit

Chats                        Messages
  j/k or ↑/↓  move             j/k or ↑/↓  select bubble
  Enter       compose          o / Enter   open attachment / link
  /           filter           s           save attachment → ~/Downloads
  Esc         clear filter     y           copy text
                               PgUp/PgDn   scroll,  g/G  top/bottom

Compose
  Enter       send             Alt+Enter / Ctrl+J   newline
  Esc         back to messages Backspace on empty   remove attachment
  Ctrl+L      redraw screen";
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
}

// ----- helpers -----

fn att_icon(mime: &str, sticker: bool) -> &'static str {
    if sticker { return "🏷"; }
    if mime.starts_with("image/") { "🖼" } else if mime.starts_with("video/") { "🎬" } else if mime.starts_with("audio/") { "🎵" } else if mime.contains("pdf") { "📄" } else if mime.contains("vcard") { "👤" } else { "📎" }
}

fn first_name(s: &str) -> String {
    s.split_whitespace().next().unwrap_or(s).to_string()
}

pub fn truncate(s: &str, w: usize) -> String {
    if s.width() <= w { return s.to_string(); }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + cw + 1 > w { break; }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

fn pad(s: &str, w: usize) -> String {
    let sw = s.width();
    if sw >= w { s.to_string() } else { format!("{s}{}", " ".repeat(w - sw)) }
}

fn local(ms: i64) -> Option<DateTime<Local>> {
    if ms <= 0 { return None; }
    Local.timestamp_millis_opt(ms).single()
}

pub fn short_time(ms: i64) -> String {
    let Some(d) = local(ms) else { return String::new() };
    let now = Local::now();
    if d.date_naive() == now.date_naive() {
        d.format("%-I:%M %p").to_string()
    } else if (now - d).num_days() < 7 {
        d.format("%a").to_string()
    } else if d.year() == now.year() {
        d.format("%b %-d").to_string()
    } else {
        d.format("%-m/%-d/%y").to_string()
    }
}

pub fn short_clock(ms: i64) -> String {
    local(ms).map(|d| d.format("%-I:%M %p").to_string()).unwrap_or_default()
}

pub fn full_time(ms: i64) -> String {
    let Some(d) = local(ms) else { return String::new() };
    let now = Local::now();
    if d.date_naive() == now.date_naive() {
        format!("Today {}", d.format("%-I:%M %p"))
    } else if (now - d).num_days() < 7 {
        d.format("%A %-I:%M %p").to_string()
    } else {
        d.format("%b %-d, %Y %-I:%M %p").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn msg(id: i64, from_me: bool, text: &str) -> Message {
        Message {
            id, guid: format!("g{id}"), chat_id: 1, sender: None, is_from_me: from_me, date: 1_700_000_000_000 + id * 1000,
            date_read: 0, date_delivered: 0, is_read: true, is_delivered: true, is_sent: true, error: 0, service: "iMessage".into(),
            subject: None, text: Some(text.into()), parts: vec![TextPart { text: text.into(), effects: vec![] }], attachments: vec![],
            reactions: vec![], kind: MessageKind::Normal, reply_to: None, thread_originator_guid: None, is_edited: false, expressive: None, is_audio: false,
        }
    }

    #[test]
    fn wide_emoji_rows_align() {
        let msgs = vec![msg(1, false, "Ok. Miss you and love you 😘"), msg(2, false, "plain")];
        let r = render_messages(&msgs, 60, false, false, None, false, false);
        let backend = TestBackend::new(60, 12);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| f.render_widget(Paragraph::new(Text::from(r.lines.clone())), f.area())).unwrap();
        let buf = term.backend().buffer().clone();
        let rows: Vec<String> = (0..12).map(|y| (0..60).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect();
        for (i, row) in rows.iter().enumerate() { eprintln!("{i:2}|{row}|"); }
        // Every bubble edge row must start at the same column as its content row.
        let top = rows[1].find('▄').unwrap();
        let bottom = rows[3].find('▀').unwrap();
        assert_eq!(top, bottom, "top/bottom edge misaligned around wide emoji");
    }
}
