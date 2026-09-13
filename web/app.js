/* imsg web client — vanilla JS, talks to imsg-server */
function mergeChatLists(existing, incoming) {
  const byId = new Map(existing.map(c => [c.id, c]));
  for (const c of incoming) byId.set(c.id, c);
  return [...byId.values()].sort((a, b) => (b.last_date - a.last_date) || (b.id - a.id));
}

(() => {
const $ = (id) => document.getElementById(id);
const els = {
  login: $('login'), loginForm: $('loginForm'), tokenInput: $('tokenInput'), loginErr: $('loginErr'),
  app: $('app'), chatList: $('chatList'), search: $('search'), newBtn: $('newBtn'), connState: $('connState'),
  chatTitle: $('chatTitle'), chatSub: $('chatSub'), messages: $('messages'), msgList: $('msgList'), loadMore: $('loadMore'),
  input: $('input'), sendBtn: $('sendBtn'), attachBtn: $('attachBtn'), fileInput: $('fileInput'), chips: $('chips'),
  emojiBtn: $('emojiBtn'), emojiPanel: $('emojiPanel'), emojiSearch: $('emojiSearch'), emojiGrid: $('emojiGrid'),
  picker: $('picker'), pickerSearch: $('pickerSearch'), pickerList: $('pickerList'), pickerClose: $('pickerClose'),
  lightbox: $('lightbox'), lightboxImg: $('lightboxImg'), toast: $('toast'), backBtn: $('backBtn'), dropHint: $('dropHint'),
};

const state = {
  token: localStorage.getItem('imsg_token') || '',
  chats: [], chatById: new Map(), contacts: [], contactByAddr: new Map(),
  currentId: null, newChat: null, // {address, name}
  messages: new Map(), hasMore: new Map(),
  pending: [], // {id, name, size, uploading}
  ws: null, info: null, searchMode: false,
  pickerSel: 0,
};

// ---------- helpers ----------
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const norm = (a) => { a = (a||'').trim(); if (a.includes('@')) return a.toLowerCase(); const d = a.replace(/\D/g,''); return (d.length===11 && d[0]==='1') ? d.slice(1) : d; };
const fmtSize = (b) => b > 1e9 ? (b/1e9).toFixed(1)+' GB' : b > 1e6 ? (b/1e6).toFixed(1)+' MB' : b > 1e3 ? Math.round(b/1e3)+' KB' : b+' B';
const sameDay = (a, b) => a.toDateString() === b.toDateString();
function fmtTime(ms) { return new Date(ms).toLocaleTimeString([], {hour: 'numeric', minute: '2-digit'}); }
function fmtListTime(ms) {
  if (!ms) return ''; const d = new Date(ms), now = new Date();
  if (sameDay(d, now)) return fmtTime(ms);
  if (now - d < 6*864e5) return d.toLocaleDateString([], {weekday: 'short'});
  if (d.getFullYear() === now.getFullYear()) return d.toLocaleDateString([], {month: 'short', day: 'numeric'});
  return d.toLocaleDateString([], {year: '2-digit', month: 'numeric', day: 'numeric'});
}
function fmtSep(ms) {
  const d = new Date(ms), now = new Date();
  if (sameDay(d, now)) return `<b>Today</b> ${fmtTime(ms)}`;
  if (now - d < 6*864e5) return `<b>${d.toLocaleDateString([], {weekday: 'long'})}</b> ${fmtTime(ms)}`;
  return `<b>${d.toLocaleDateString([], {weekday: 'short', month: 'short', day: 'numeric', year: d.getFullYear()!==now.getFullYear()?'numeric':undefined})}</b> ${fmtTime(ms)}`;
}
function toast(msg, ms = 2500) { els.toast.textContent = msg; els.toast.classList.remove('hidden'); clearTimeout(toast.t); toast.t = setTimeout(() => els.toast.classList.add('hidden'), ms); }
function initials(name) { const p = (name||'').trim().split(/\s+/); if (!p[0]) return '?'; if (/^[+\d(]/.test(p[0])) return '#'; return (p[0][0] + (p[1]?.[0]||'')).toUpperCase(); }
function chatTitle(c) { if (c.display_name) return c.display_name; if (!c.participants.length) return c.identifier; return c.participants.map(p => p.name || p.address).join(', '); }
function avatarHtml(c) {
  if (c.is_group) return `<div class="avatar group">👥</div>`;
  const p = c.participants[0];
  if (p && p.contact_id != null) { const ct = state.contacts.find(x => x.id === p.contact_id); if (ct && ct.has_photo) return `<div class="avatar"><img src="/api/contacts/${ct.id}/photo" alt=""></div>`; }
  return `<div class="avatar">${esc(initials(chatTitle(c)))}</div>`;
}
function linkify(text) {
  return esc(text).replace(/(https?:\/\/[^\s<]+[^\s<.,;:!?)\]])/g, '<a href="$1" target="_blank" rel="noopener">$1</a>');
}

// ---------- API ----------
async function api(path, opts = {}) {
  const headers = Object.assign({ Authorization: 'Bearer ' + state.token }, opts.headers || {});
  const r = await fetch(path, Object.assign({}, opts, { headers }));
  if (r.status === 401) { showLogin('Invalid token'); throw new Error('unauthorized'); }
  if (!r.ok) throw new Error(`${path}: ${r.status}`);
  return r.json();
}

// ---------- login ----------
function showLogin(err = '') { els.login.classList.remove('hidden'); els.loginErr.textContent = err; els.tokenInput.focus(); }
els.loginForm.addEventListener('submit', async (e) => {
  e.preventDefault();
  state.token = els.tokenInput.value.trim();
  try { await api('/api/info'); localStorage.setItem('imsg_token', state.token); document.cookie = `imsg_token=${encodeURIComponent(state.token)}; path=/; SameSite=Lax; max-age=31536000`; els.login.classList.add('hidden'); boot(); }
  catch (e) { els.loginErr.textContent = 'Could not connect: ' + e.message; }
});

// ---------- boot ----------
async function boot() {
  document.cookie = `imsg_token=${encodeURIComponent(state.token)}; path=/; SameSite=Lax; max-age=31536000`;
  try { state.info = await api('/api/info'); } catch (e) { return; }
  document.title = `Messages · ${state.info.host}`;
  loadChats(); loadContacts(); connectWs();
  if ('Notification' in window && Notification.permission === 'default') { document.addEventListener('click', () => Notification.requestPermission(), { once: true }); }
}
async function loadChats() { try { mergeChats(await api('/api/chats?limit=300')); } catch (e) { toast('Failed to load chats'); } }
async function loadContacts() { try { state.contacts = await api('/api/contacts'); state.contactByAddr.clear(); for (const c of state.contacts) for (const a of [...c.phones, ...c.emails]) state.contactByAddr.set(norm(a), c); renderChats(); } catch (e) {} }
function mergeChats(chats) {
  // Websocket chat events contain only the newest 100 chats. Keep older
  // conversations already loaded by the HTTP snapshot and merge updates by ID.
  state.chats = mergeChatLists(state.chats, chats);
  state.chatById = new Map(state.chats.map(c => [c.id, c]));
  renderChats(); if (state.currentId) { renderHeader(); ensureTail(state.currentId); } updateTitle();
  if (state.newChat) { const want = norm(state.newChat.address); const c = state.chats.find(x => !x.is_group && x.participants.length === 1 && norm(x.participants[0].address) === want); if (c) { state.newChat = null; openChat(c.id); } }
}
// Safety net: if the chat list says there is something newer than what we have loaded, fetch the tail.
async function ensureTail(id) {
  const c = state.chatById.get(id), list = state.messages.get(id); if (!c || !list) return;
  const newest = list.length ? list[list.length - 1].date : 0;
  if (c.last_date <= newest + 500) return;
  if (ensureTail.busy) return; ensureTail.busy = true;
  try { const page = await api(`/api/chats/${id}/messages?limit=30`); mergeMessages(id, page.messages); if (state.currentId === id) renderMessages(true); }
  catch (e) {} finally { ensureTail.busy = false; }
}
function mergeMessages(chatId, msgs) {
  const list = state.messages.get(chatId); if (!list) return;
  for (const m of msgs) { const i = list.findIndex(x => x.id === m.id); if (i >= 0) list[i] = m; else list.push(m); }
  list.sort((a, b) => a.id - b.id);
}
function updateTitle() { const n = state.chats.reduce((a, c) => a + (c.unread > 0 ? 1 : 0), 0); document.title = (n ? `(${n}) ` : '') + `Messages · ${state.info?.host || ''}`; }

// ---------- websocket ----------
function connectWs() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws?token=${encodeURIComponent(state.token)}`);
  state.ws = ws;
  ws.onopen = () => { els.connState.textContent = `live · ${state.info?.host || ''}`; els.connState.className = 'conn live'; };
  ws.onclose = () => { els.connState.textContent = 'reconnecting…'; els.connState.className = 'conn off'; setTimeout(connectWs, 2000); };
  ws.onmessage = (ev) => {
    let d; try { d = JSON.parse(ev.data); } catch { return; }
    if (d.event === 'messages') onNewMessages(d.messages);
    else if (d.event === 'chats') mergeChats(d.chats);
  };
}
function onNewMessages(msgs) {
  for (const m of msgs) {
    const list = state.messages.get(m.chat_id);
    if (list) { const i = list.findIndex(x => x.id === m.id); if (i >= 0) list[i] = m; else { list.push(m); list.sort((a, b) => a.id - b.id); } }
    if (!m.is_from_me && m.chat_id !== state.currentId && document.hidden && 'Notification' in window && Notification.permission === 'granted' && m.kind.type === 'normal') {
      const c = state.chatById.get(m.chat_id); new Notification((m.sender?.name || m.sender?.address || 'Message') + (c?.is_group ? ` · ${chatTitle(c)}` : ''), { body: m.text || (m.attachments.length ? '📎 Attachment' : '') });
    }
  }
  if (msgs.some(m => m.chat_id === state.currentId)) renderMessages(true);
}

// ---------- sidebar ----------
function renderChats() {
  if (state.searchMode) return;
  const q = els.search.value.trim().toLowerCase();
  const frag = document.createDocumentFragment();
  if (state.newChat) {
    const d = document.createElement('div'); d.className = 'chat active'; d.innerHTML = `<div class="avatar">✎</div><div class="chat-mid"><div class="chat-name">${esc(state.newChat.name)}</div><div class="chat-prev">New message</div></div>`; frag.appendChild(d);
  }
  for (const c of state.chats) {
    const title = chatTitle(c);
    if (q && !title.toLowerCase().includes(q) && !c.participants.some(p => p.address.toLowerCase().includes(q)) && !(c.last_preview||'').toLowerCase().includes(q)) continue;
    const d = document.createElement('div');
    d.className = 'chat' + (c.id === state.currentId && !state.newChat ? ' active' : '');
    d.dataset.id = c.id;
    const prev = (c.last_from_me && c.last_preview ? 'You: ' : '') + (c.last_preview || '');
    d.innerHTML = `${avatarHtml(c)}<div class="chat-mid"><div class="chat-name">${c.unread ? '<span class="unread"></span>' : ''}${esc(title)}</div><div class="chat-prev">${esc(prev.replace(/\n/g, ' '))}</div></div><div class="chat-time">${fmtListTime(c.last_date)}</div>`;
    d.onclick = () => openChat(c.id);
    frag.appendChild(d);
  }
  els.chatList.replaceChildren(frag);
}
els.search.addEventListener('input', () => { state.searchMode = false; renderChats(); });
els.search.addEventListener('keydown', async (e) => {
  if (e.key === 'Escape') { els.search.value = ''; state.searchMode = false; renderChats(); return; }
  if (e.key !== 'Enter') return;
  const q = els.search.value.trim(); if (!q) return;
  state.searchMode = true; els.chatList.innerHTML = '<div class="conn">searching…</div>';
  try {
    const res = await api(`/api/search?limit=100&q=${encodeURIComponent(q)}`);
    const frag = document.createDocumentFragment();
    if (!res.length) { const d = document.createElement('div'); d.className = 'conn'; d.textContent = 'No results'; frag.appendChild(d); }
    for (const m of res) {
      const c = state.chatById.get(m.chat_id); const d = document.createElement('div'); d.className = 'search-hit';
      const who = m.is_from_me ? 'You' : (m.sender?.name || m.sender?.address || '');
      const re = new RegExp(q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'ig');
      d.innerHTML = `<div class="sh-meta">${esc(c ? chatTitle(c) : '')} · ${who ? esc(who) + ' · ' : ''}${fmtListTime(m.date)}</div><div class="sh-text">${esc(m.text || '').replace(re, (x) => `<mark>${esc(x)}</mark>`)}</div>`;
      d.onclick = () => { state.searchMode = false; els.search.value = ''; openChat(m.chat_id, m.id); };
      frag.appendChild(d);
    }
    els.chatList.replaceChildren(frag);
  } catch (e) { toast('Search failed'); }
});

// ---------- chat view ----------
async function openChat(id, highlightId) {
  state.newChat = null; state.currentId = id; els.app.classList.add('show-chat');
  renderChats(); renderHeader();
  if (!state.messages.has(id)) {
    els.msgList.innerHTML = '<div class="announce">Loading…</div>';
    try { const page = await api(`/api/chats/${id}/messages?limit=60`); state.messages.set(id, page.messages); state.hasMore.set(id, page.has_more); }
    catch (e) { els.msgList.innerHTML = '<div class="announce">Failed to load</div>'; return; }
  }
  if (state.currentId !== id) return;
  renderMessages(true);
  if (highlightId) { const el = els.msgList.querySelector(`[data-id="${highlightId}"]`); if (el) { el.scrollIntoView({ block: 'center' }); el.style.outline = '2px solid var(--accent)'; setTimeout(() => el.style.outline = '', 2000); } }
  els.input.focus();
}
function renderHeader() {
  if (state.newChat) { els.chatTitle.textContent = 'New message to ' + state.newChat.name; els.chatSub.textContent = state.newChat.address; return; }
  const c = state.chatById.get(state.currentId); if (!c) return;
  els.chatTitle.textContent = chatTitle(c);
  els.chatSub.textContent = c.is_group ? `${c.service} · ${c.participants.map(p => p.name || p.address).join(', ')}` : `${c.service} · ${c.participants[0]?.address || c.identifier}`;
}
els.loadMore.onclick = loadOlder;
async function loadOlder() {
  const id = state.currentId; const list = state.messages.get(id); if (!list || !list.length || !state.hasMore.get(id)) return;
  els.loadMore.textContent = 'Loading…';
  try {
    const page = await api(`/api/chats/${id}/messages?limit=60&before=${list[0].id}`);
    const prevH = els.messages.scrollHeight;
    state.messages.set(id, page.messages.concat(list)); state.hasMore.set(id, page.has_more);
    renderMessages(false); els.messages.scrollTop += els.messages.scrollHeight - prevH;
  } catch (e) { toast('Failed to load'); }
  els.loadMore.textContent = 'Load earlier messages';
}
els.messages.addEventListener('scroll', () => { if (els.messages.scrollTop < 40 && state.hasMore.get(state.currentId)) loadOlder(); });

function partHtml(p) {
  let h = linkify(p.text);
  let cls = [];
  for (const e of p.effects) for (const t of e.split(',')) {
    if (t === 'bold') h = `<b>${h}</b>`; else if (t === 'italic') h = `<i>${h}</i>`; else if (t === 'strikethrough') h = `<s>${h}</s>`; else if (t === 'underline') h = `<u>${h}</u>`;
    else if (t === 'otp') cls.push('otp'); else if (t.startsWith('mention:')) cls.push('mention');
    else if (t.startsWith('link:')) { const u = t.slice(5); if (!h.includes('<a ')) h = `<a href="${esc(u)}" target="_blank" rel="noopener">${h}</a>`; }
  }
  return cls.length ? `<span class="${cls.join(' ')}">${h}</span>` : h;
}
function attHtml(a, m) {
  if (!a.available) return `<span class="file-chip"><span class="ico">📎</span><span class="nm">${esc(a.name)}</span><span class="sz">${m.is_from_me && Date.now() - m.date < 600e3 ? 'sending…' : 'not on this Mac'}</span></span>`;
  const u = a.url;
  if (a.mime.startsWith('image/')) return `<img class="att-img" src="${u}?preview=1" data-full="${u}" alt="${esc(a.name)}" loading="lazy">`;
  if (a.mime.startsWith('video/')) return `<video controls preload="metadata" src="${u}"></video>`;
  if (a.mime.startsWith('audio/')) return `<audio controls preload="metadata" src="${u}"></audio>`;
  const ico = a.mime.includes('pdf') ? '📄' : a.mime.includes('vcard') ? '👤' : '📎';
  return `<a class="file-chip" href="${u}?download=1"><span class="ico">${ico}</span><span class="nm">${esc(a.name)}</span><span class="sz">${fmtSize(a.size)}</span></a>`;
}
function renderMessages(stickBottom) {
  const c = state.chatById.get(state.currentId);
  const list = state.messages.get(state.currentId) || [];
  els.loadMore.classList.toggle('hidden', !state.hasMore.get(state.currentId));
  const atBottom = els.messages.scrollHeight - els.messages.scrollTop - els.messages.clientHeight < 80;
  const out = [];
  let lastDate = 0, lastFromMeIdx = -1;
  list.forEach((m, i) => { if (m.is_from_me) lastFromMeIdx = i; });
  for (let i = 0; i < list.length; i++) {
    const m = list[i], prev = list[i - 1], next = list[i + 1];
    if (m.date - lastDate > 3600e3) out.push(`<div class="sep">${fmtSep(m.date)}</div>`);
    lastDate = m.date;
    if (m.kind.type === 'announcement') { out.push(`<div class="announce" data-id="${m.id}">${esc(m.kind.text)}</div>`); continue; }
    const sameSenderPrev = prev && prev.kind.type !== 'announcement' && prev.is_from_me === m.is_from_me && (prev.sender?.handle_id === m.sender?.handle_id) && m.date - prev.date < 3600e3;
    const sameSenderNext = next && next.kind.type !== 'announcement' && next.is_from_me === m.is_from_me && (next.sender?.handle_id === m.sender?.handle_id) && next.date - m.date < 3600e3;
    const cls = ['row', m.is_from_me ? 'me' : 'them', sameSenderNext ? '' : 'last', (m.service !== 'iMessage') ? 'sms' : ''].join(' ');
    let inner = '';
    if (c?.is_group && !m.is_from_me && !sameSenderPrev) inner += `<div class="sender">${esc(m.sender?.name || m.sender?.address || 'Unknown')}</div>`;
    if (m.reply_to) inner += `<div class="quote"><b>${esc(m.reply_to.sender_name || '')}</b> ${esc(m.reply_to.preview || '')}</div>`;
    let body = '';
    if (m.subject) body += `<span class="subject">${esc(m.subject)}</span>`;
    const imgs = m.attachments.filter(a => a.available && a.mime.startsWith('image/'));
    const isApp = m.kind.type === 'app';
    if (m.kind.type === 'unsent') body += `<span class="unsent">This message was unsent</span>`;
    else if (isApp) {
      const k = m.kind; const img = imgs[0];
      body += `<a class="link-card" href="${esc(k.url || '#')}" target="_blank" rel="noopener">${img ? `<img src="${img.url}?preview=1" alt="">` : ''}<div class="lc-body"><div class="lc-title">${esc(k.title || k.summary || k.bundle_id.split('.').pop())}</div>${k.summary && k.title ? `<div class="lc-sum">${esc(k.summary)}</div>` : ''}${k.url ? `<div class="lc-url">${esc(k.url.replace(/^https?:\/\//, ''))}</div>` : ''}</div></a>`;
    } else {
      body += m.parts.map(partHtml).join('');
      for (const a of m.attachments) { if (a.name.endsWith('.pluginPayloadAttachment')) continue; body += attHtml(a, m); }
      if (m.is_audio && !m.attachments.length) body += '🎤 Audio message';
      if (!body) body += `<span class="unsent">${m.is_from_me && Date.now() - m.date < 600e3 ? 'Sending…' : '(no content)'}</span>`;
    }
    if (m.expressive) body += `<span class="fx">✨ Sent with ${esc(m.expressive)}</span>`;
    if (m.is_edited) body += `<span class="edited">Edited</span>`;
    const imgOnly = !isApp && imgs.length && !m.text && m.attachments.length === imgs.length && !m.subject;
    inner += `<div class="bubble${imgOnly ? ' img-only' : ''}" title="${fmtTime(m.date)}">${body}</div>`;
    if (m.reactions.length) inner += `<div class="reacts">${m.reactions.map(r => `<span class="react${r.from_me ? ' mine' : ''}" title="${esc(r.from_me ? 'You' : (r.sender?.name || r.sender?.address || ''))}">${esc(r.emoji)}</span>`).join('')}</div>`;
    if (!sameSenderNext) inner += `<div class="time">${fmtTime(m.date)}</div>`;
    if (i === lastFromMeIdx) {
      const s = m.error ? 'Not Delivered' : m.date_read ? `Read ${fmtTime(m.date_read)}` : (m.is_delivered || m.date_delivered) ? 'Delivered' : m.is_sent ? 'Sent' : '';
      if (s) inner += `<div class="status${m.error ? ' err' : ''}">${s}</div>`;
    }
    out.push(`<div class="${cls}" data-id="${m.id}">${inner}</div>`);
  }
  if (!list.length && !state.newChat) out.push('<div class="announce">No messages</div>');
  els.msgList.innerHTML = out.join('');
  if (stickBottom && (atBottom || true)) els.messages.scrollTop = els.messages.scrollHeight;
}
els.msgList.addEventListener('click', (e) => {
  const img = e.target.closest('img.att-img'); if (img) { els.lightboxImg.src = img.dataset.full; els.lightbox.classList.remove('hidden'); }
});
els.lightbox.onclick = () => { els.lightbox.classList.add('hidden'); els.lightboxImg.src = ''; };
els.backBtn.onclick = () => els.app.classList.remove('show-chat');

// ---------- composer ----------
function autosize() { els.input.style.height = 'auto'; els.input.style.height = Math.min(els.input.scrollHeight, 160) + 'px'; }
els.input.addEventListener('input', () => { autosize(); expandShortcode(); });
els.input.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); } });
els.sendBtn.onclick = send;
async function send() {
  const text = els.input.value.trim();
  if (!text && !state.pending.length) return;
  if (state.pending.some(p => p.uploading)) { toast('Still uploading…'); return; }
  const req = { text: text || null, uploads: state.pending.map(p => p.id) };
  if (state.newChat) req.to = [state.newChat.address]; else { const c = state.chatById.get(state.currentId); if (!c) return; req.chat_guid = c.guid; }
  els.input.value = ''; autosize(); state.pending = []; renderChips(); els.sendBtn.disabled = true;
  try { const r = await api('/api/send', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(req) }); if (!r.ok) toast('Send failed: ' + (r.detail || '')); }
  catch (e) { toast('Send failed: ' + e.message); }
  els.sendBtn.disabled = false; els.input.focus();
}
// attachments
els.attachBtn.onclick = () => els.fileInput.click();
els.fileInput.onchange = () => { for (const f of els.fileInput.files) upload(f); els.fileInput.value = ''; };
async function upload(file) {
  const chip = { id: null, name: file.name, size: file.size, uploading: true }; state.pending.push(chip); renderChips();
  try { const fd = new FormData(); fd.append('file', file, file.name); const r = await api('/api/upload', { method: 'POST', body: fd }); chip.id = r.id; chip.uploading = false; }
  catch (e) { state.pending = state.pending.filter(p => p !== chip); toast('Upload failed'); }
  renderChips();
}
function renderChips() {
  els.chips.classList.toggle('hidden', !state.pending.length);
  els.chips.replaceChildren(...state.pending.map((p, i) => { const d = document.createElement('div'); d.className = 'chip' + (p.uploading ? ' uploading' : ''); d.innerHTML = `📎 ${esc(p.name)} <span class="sz">${fmtSize(p.size)}</span> <button title="Remove">✕</button>`; d.querySelector('button').onclick = () => { state.pending.splice(i, 1); renderChips(); }; return d; }));
}
document.addEventListener('paste', (e) => { for (const it of e.clipboardData?.items || []) { if (it.kind === 'file') { const f = it.getAsFile(); if (f) upload(new File([f], f.name || `pasted-${Date.now()}.png`, { type: f.type })); } } });
let dragN = 0;
document.addEventListener('dragenter', (e) => { e.preventDefault(); dragN++; els.dropHint.classList.remove('hidden'); });
document.addEventListener('dragleave', () => { if (--dragN <= 0) { dragN = 0; els.dropHint.classList.add('hidden'); } });
document.addEventListener('dragover', (e) => e.preventDefault());
document.addEventListener('drop', (e) => { e.preventDefault(); dragN = 0; els.dropHint.classList.add('hidden'); for (const f of e.dataTransfer.files) upload(f); });

// ---------- emoji ----------
const EMOJI = [
 ['😀','grinning smile happy'],['😃','smiley happy'],['😄','smile happy'],['😁','grin beam'],['😆','laughing satisfied'],['😅','sweat smile'],['🤣','rofl rolling'],['😂','joy tears laugh'],['🙂','slight smile'],['🙃','upside down'],['😉','wink'],['😊','blush'],['😇','innocent halo'],['🥰','love hearts'],['😍','heart eyes'],['🤩','star struck'],['😘','kiss'],['😗','kissing'],['😚','kissing closed'],['😙','kissing smiling'],['🥲','tear smile'],['😋','yum'],['😛','tongue'],['😜','wink tongue'],['🤪','zany'],['😝','squint tongue'],['🤑','money'],['🤗','hug'],['🤭','hand mouth'],['🤫','shush'],['🤔','thinking'],['🫡','salute'],['🤐','zipper'],['🤨','raised eyebrow'],['😐','neutral'],['😑','expressionless'],['😶','no mouth'],['😏','smirk'],['😒','unamused'],['🙄','eye roll'],['😬','grimace'],['🤥','lying'],['😌','relieved'],['😔','pensive'],['😪','sleepy'],['🤤','drool'],['😴','sleeping'],['😷','mask'],['🤒','thermometer sick'],['🤕','bandage'],['🤢','nauseated'],['🤮','vomit'],['🤧','sneeze'],['🥵','hot'],['🥶','cold'],['🥴','woozy'],['😵','dizzy'],['🤯','exploding head'],['🤠','cowboy'],['🥳','party'],['🥸','disguise'],['😎','sunglasses cool'],['🤓','nerd'],['🧐','monocle'],['😕','confused'],['😟','worried'],['🙁','frown'],['☹️','frowning'],['😮','open mouth'],['😯','hushed'],['😲','astonished'],['😳','flushed'],['🥺','pleading'],['🥹','holding tears'],['😦','frowning open'],['😧','anguished'],['😨','fearful'],['😰','anxious sweat'],['😥','sad relieved'],['😢','cry'],['😭','sob'],['😱','scream'],['😖','confounded'],['😣','persevere'],['😞','disappointed'],['😓','sweat'],['😩','weary'],['😫','tired'],['🥱','yawn'],['😤','triumph'],['😡','rage angry'],['😠','angry'],['🤬','cursing'],['😈','devil'],['👿','imp'],['💀','skull'],['☠️','skull crossbones'],['💩','poop'],['🤡','clown'],['👹','ogre'],['👺','goblin'],['👻','ghost'],['👽','alien'],['🤖','robot'],['😺','cat smile'],['😸','cat grin'],['😹','cat joy'],['😻','cat heart eyes'],['🙈','see no evil'],['🙉','hear no evil'],['🙊','speak no evil'],
 ['❤️','red heart love'],['🧡','orange heart'],['💛','yellow heart'],['💚','green heart'],['💙','blue heart'],['💜','purple heart'],['🖤','black heart'],['🤍','white heart'],['🤎','brown heart'],['💔','broken heart'],['❤️‍🔥','heart fire'],['💕','two hearts'],['💞','revolving hearts'],['💓','heartbeat'],['💗','growing heart'],['💖','sparkling heart'],['💘','cupid'],['💝','gift heart'],['💯','100 hundred'],['💢','anger'],['💥','boom'],['💫','dizzy'],['💦','sweat drops'],['💨','dash'],['🕳️','hole'],['💬','speech'],['👁️‍🗨️','eye speech'],['🗨️','left speech'],['💤','zzz sleep'],
 ['👋','wave hello'],['🤚','raised back hand'],['🖐️','hand fingers'],['✋','raised hand stop'],['🖖','vulcan'],['👌','ok'],['🤌','pinched'],['🤏','pinch'],['✌️','victory peace'],['🤞','fingers crossed'],['🫰','snap'],['🤟','love you'],['🤘','rock'],['🤙','call me'],['👈','point left'],['👉','point right'],['👆','point up'],['🖕','middle finger'],['👇','point down'],['☝️','index up'],['👍','thumbs up like'],['👎','thumbs down'],['✊','fist'],['👊','punch'],['🤛','left fist'],['🤜','right fist'],['👏','clap'],['🙌','raised hands hooray'],['🫶','heart hands'],['👐','open hands'],['🤲','palms up'],['🤝','handshake'],['🙏','pray thanks please'],['✍️','writing'],['💅','nail polish'],['🤳','selfie'],['💪','muscle strong'],['🦾','mechanical arm'],['🧠','brain'],['👀','eyes'],['👁️','eye'],['👅','tongue'],['👄','mouth'],
 ['🐶','dog'],['🐱','cat'],['🐭','mouse'],['🐹','hamster'],['🐰','rabbit'],['🦊','fox'],['🐻','bear'],['🐼','panda'],['🐨','koala'],['🐯','tiger'],['🦁','lion'],['🐮','cow'],['🐷','pig'],['🐸','frog'],['🐵','monkey'],['🐔','chicken'],['🐧','penguin'],['🐦','bird'],['🦆','duck'],['🦅','eagle'],['🦉','owl'],['🐺','wolf'],['🐴','horse'],['🦄','unicorn'],['🐝','bee'],['🐛','bug'],['🦋','butterfly'],['🐌','snail'],['🐢','turtle'],['🐍','snake'],['🦖','trex dinosaur'],['🐙','octopus'],['🦀','crab'],['🐟','fish'],['🐬','dolphin'],['🐳','whale'],['🦈','shark'],['🐊','crocodile'],['🐘','elephant'],['🦒','giraffe'],['🐕','dog'],['🐈','cat'],['🌵','cactus'],['🎄','christmas tree'],['🌲','evergreen'],['🌴','palm'],['🌱','seedling'],['🍀','clover luck'],['🍁','maple'],['🍂','fallen leaf'],['🌸','cherry blossom'],['🌹','rose'],['🌻','sunflower'],['🌞','sun face'],['🌝','full moon face'],['🌙','crescent moon'],['⭐','star'],['🌟','glowing star'],['✨','sparkles'],['⚡','zap lightning'],['🔥','fire lit'],['🌈','rainbow'],['☀️','sun'],['⛅','cloud sun'],['☁️','cloud'],['🌧️','rain'],['⛈️','storm'],['❄️','snow'],['☃️','snowman'],['🌊','wave ocean'],
 ['🍎','apple'],['🍊','orange'],['🍋','lemon'],['🍌','banana'],['🍉','watermelon'],['🍇','grapes'],['🍓','strawberry'],['🫐','blueberries'],['🍒','cherries'],['🍑','peach'],['🥭','mango'],['🍍','pineapple'],['🥥','coconut'],['🥑','avocado'],['🍆','eggplant'],['🥕','carrot'],['🌽','corn'],['🌶️','pepper hot'],['🥦','broccoli'],['🍞','bread'],['🥐','croissant'],['🥯','bagel'],['🧀','cheese'],['🥚','egg'],['🍳','cooking'],['🥓','bacon'],['🍔','burger'],['🍟','fries'],['🍕','pizza'],['🌭','hotdog'],['🥪','sandwich'],['🌮','taco'],['🌯','burrito'],['🍜','ramen'],['🍣','sushi'],['🍦','ice cream'],['🍩','donut'],['🍪','cookie'],['🎂','birthday cake'],['🍰','cake'],['🧁','cupcake'],['🍫','chocolate'],['🍿','popcorn'],['☕','coffee'],['🍵','tea'],['🧃','juice'],['🥤','cup straw'],['🍺','beer'],['🍻','cheers beers'],['🥂','clink champagne'],['🍷','wine'],['🥃','whiskey'],['🍸','cocktail'],
 ['⚽','soccer'],['🏀','basketball'],['🏈','football'],['⚾','baseball'],['🎾','tennis'],['🏐','volleyball'],['🎱','8ball'],['🏓','ping pong'],['🥊','boxing'],['⛳','golf'],['🎣','fishing'],['🎿','ski'],['🏆','trophy'],['🥇','gold medal'],['🎯','target dart'],['🎮','video game'],['🕹️','joystick'],['🎲','dice'],['🧩','puzzle'],['🎨','art palette'],['🎬','clapper movie'],['🎤','mic'],['🎧','headphones'],['🎵','music note'],['🎶','notes'],['🎸','guitar'],['🎹','piano'],['🥁','drum'],['🎺','trumpet'],['🎻','violin'],
 ['🚗','car'],['🚕','taxi'],['🚙','suv'],['🚌','bus'],['🚎','trolley'],['🏎️','race car'],['🚓','police car'],['🚑','ambulance'],['🚒','fire engine'],['🚚','truck'],['🚜','tractor'],['🏍️','motorcycle'],['🚲','bike'],['🛴','scooter'],['✈️','airplane'],['🚀','rocket'],['🛸','ufo'],['🚁','helicopter'],['⛵','sailboat'],['🚢','ship'],['🚂','train'],['🚇','metro'],['🏠','house home'],['🏡','house garden'],['🏢','office'],['🏥','hospital'],['🏫','school'],['⛪','church'],['🗽','statue liberty'],['🗼','tower'],['🏖️','beach'],['🏕️','camping'],['🌋','volcano'],['🗻','mount fuji'],['⛰️','mountain'],
 ['⌚','watch'],['📱','phone mobile'],['💻','laptop'],['🖥️','desktop'],['🖨️','printer'],['⌨️','keyboard'],['🖱️','mouse'],['💾','floppy'],['💿','cd'],['📷','camera'],['📸','camera flash'],['📹','video camera'],['🎥','movie camera'],['📺','tv'],['📻','radio'],['⏰','alarm clock'],['⏳','hourglass'],['🔋','battery'],['🔌','plug'],['💡','bulb idea'],['🔦','flashlight'],['🕯️','candle'],['💰','money bag'],['💵','dollar'],['💳','credit card'],['💎','gem diamond'],['🔧','wrench'],['🔨','hammer'],['🛠️','tools'],['⚙️','gear'],['🔒','lock'],['🔓','unlock'],['🔑','key'],['🚪','door'],['🛏️','bed'],['🛋️','couch'],['🚽','toilet'],['🚿','shower'],['🛁','bath'],['🧻','toilet paper'],['🧹','broom'],['🧺','basket'],['📦','package box'],['📫','mailbox'],['📮','postbox'],['✉️','envelope mail'],['📧','email'],['📝','memo note'],['📅','calendar'],['📌','pin'],['📎','paperclip'],['✂️','scissors'],['📚','books'],['📖','book'],['🔍','magnifier search'],['🔗','link'],['🧲','magnet'],['🧪','test tube'],['🔬','microscope'],['💊','pill'],['💉','syringe'],['🩹','bandage'],['🧬','dna'],
 ['✅','check mark'],['❌','cross x'],['❓','question'],['❗','exclamation'],['‼️','double exclamation'],['⚠️','warning'],['🚫','prohibited'],['♻️','recycle'],['🔞','18'],['💤','zzz'],['🔔','bell'],['🔕','bell off'],['🔊','speaker loud'],['🔇','muted'],['📢','loudspeaker'],['🎉','tada party'],['🎊','confetti'],['🎈','balloon'],['🎁','gift present'],['🎀','ribbon'],['🏁','checkered flag'],['🚩','flag red'],['🏳️‍🌈','rainbow flag'],['🇺🇸','usa flag'],['➡️','arrow right'],['⬅️','arrow left'],['⬆️','arrow up'],['⬇️','arrow down'],['🔄','refresh'],['🆗','ok button'],['🆕','new'],['🆒','cool'],['🔝','top'],['🔚','end'],['♥️','heart suit'],['♠️','spade'],['♦️','diamond'],['♣️','club'],['🃏','joker'],['🀄','mahjong'],['🎴','flower cards'],['☮️','peace'],['☯️','yin yang'],['✝️','cross'],['☪️','star crescent'],['🕉️','om'],['♾️','infinity'],['©️','copyright'],['®️','registered'],['™️','trademark'],['🔴','red circle'],['🟠','orange circle'],['🟡','yellow circle'],['🟢','green circle'],['🔵','blue circle'],['🟣','purple circle'],['⚫','black circle'],['⚪','white circle'],['🟥','red square'],['🟩','green square'],['🟦','blue square'],['⬛','black square'],['⬜','white square'],
];
function renderEmoji() {
  const q = els.emojiSearch.value.trim().toLowerCase();
  const list = q ? EMOJI.filter(([e, n]) => n.includes(q)) : EMOJI;
  els.emojiGrid.replaceChildren(...list.slice(0, 400).map(([e, n]) => { const b = document.createElement('button'); b.textContent = e; b.title = n; b.onclick = () => { insertAtCursor(e); }; return b; }));
}
function insertAtCursor(s) { const i = els.input; const st = i.selectionStart, en = i.selectionEnd; i.value = i.value.slice(0, st) + s + i.value.slice(en); i.selectionStart = i.selectionEnd = st + s.length; i.focus(); autosize(); }
els.emojiBtn.onclick = () => { els.emojiPanel.classList.toggle('hidden'); if (!els.emojiPanel.classList.contains('hidden')) { renderEmoji(); els.emojiSearch.focus(); } };
els.emojiSearch.addEventListener('input', renderEmoji);
document.addEventListener('click', (e) => { if (!els.emojiPanel.contains(e.target) && e.target !== els.emojiBtn) els.emojiPanel.classList.add('hidden'); });
const SHORT = { smile: '😄', grin: '😁', joy: '😂', rofl: '🤣', wink: '😉', blush: '😊', heart: '❤️', heart_eyes: '😍', kiss: '😘', thinking: '🤔', thumbsup: '👍', '+1': '👍', thumbsdown: '👎', '-1': '👎', ok_hand: '👌', pray: '🙏', clap: '👏', fire: '🔥', tada: '🎉', 100: '💯', eyes: '👀', sob: '😭', cry: '😢', laughing: '😆', sweat_smile: '😅', sunglasses: '😎', rage: '😡', skull: '💀', poop: '💩', ghost: '👻', star: '⭐', sparkles: '✨', zap: '⚡', check: '✅', x: '❌', warning: '⚠️', rocket: '🚀', coffee: '☕', beer: '🍺', pizza: '🍕', taco: '🌮', cake: '🎂', gift: '🎁', wave: '👋', muscle: '💪', facepalm: '🤦', shrug: '🤷', crossed_fingers: '🤞', pleading: '🥺', melting: '🫠', partying: '🥳', hug: '🤗', mask: '😷', sleeping: '😴', yawn: '🥱', dog: '🐶', cat: '🐱', unicorn: '🦄', sun: '☀️', rain: '🌧️', snow: '❄️', car: '🚗', house: '🏠', phone: '📱', money: '💰', pill: '💊', bell: '🔔', music: '🎵', camera: '📷', calendar: '📅', pin: '📌', link: '🔗', lock: '🔒', key: '🔑', bulb: '💡' };
function expandShortcode() {
  const i = els.input, pos = i.selectionStart, before = i.value.slice(0, pos);
  const m = before.match(/:([a-z0-9_+\-]+):$/i); if (!m) return;
  const e = SHORT[m[1].toLowerCase()]; if (!e) return;
  i.value = before.slice(0, -m[0].length) + e + i.value.slice(pos); i.selectionStart = i.selectionEnd = pos - m[0].length + e.length;
}

// ---------- new message picker ----------
els.newBtn.onclick = () => { els.picker.classList.remove('hidden'); els.pickerSearch.value = ''; state.pickerSel = 0; renderPicker(); setTimeout(() => els.pickerSearch.focus(), 50); };
els.pickerClose.onclick = () => els.picker.classList.add('hidden');
els.picker.addEventListener('click', (e) => { if (e.target === els.picker) els.picker.classList.add('hidden'); });
function pickerMatches() {
  const q = els.pickerSearch.value.trim(), ql = q.toLowerCase(); const out = [];
  if (q && (q.includes('@') || q.replace(/\D/g, '').length >= 7)) out.push({ name: `Send to "${q}"`, address: q, raw: true });
  for (const c of state.contacts) {
    if (ql && !c.name.toLowerCase().includes(ql) && ![...c.phones, ...c.emails].some(a => a.toLowerCase().includes(ql))) continue;
    for (const a of [...c.phones, ...c.emails]) out.push({ name: c.name, address: a, contact: c });
    if (out.length > 150) break;
  }
  return out;
}
function renderPicker() {
  const m = pickerMatches();
  els.pickerList.replaceChildren(...m.map((x, i) => { const d = document.createElement('div'); d.className = 'pick' + (i === state.pickerSel ? ' sel' : ''); d.innerHTML = `${x.contact?.has_photo ? `<div class="avatar"><img src="/api/contacts/${x.contact.id}/photo" alt=""></div>` : `<div class="avatar">${esc(initials(x.name))}</div>`}<div><div class="pn">${esc(x.name)}</div><div class="pa">${esc(x.address)}</div></div>`; d.onclick = () => startConversation(x); return d; }));
}
els.pickerSearch.addEventListener('input', () => { state.pickerSel = 0; renderPicker(); });
els.pickerSearch.addEventListener('keydown', (e) => {
  const m = pickerMatches();
  if (e.key === 'ArrowDown') { e.preventDefault(); state.pickerSel = Math.min(state.pickerSel + 1, m.length - 1); renderPicker(); }
  else if (e.key === 'ArrowUp') { e.preventDefault(); state.pickerSel = Math.max(state.pickerSel - 1, 0); renderPicker(); }
  else if (e.key === 'Enter') { e.preventDefault(); if (m[state.pickerSel]) startConversation(m[state.pickerSel]); }
  else if (e.key === 'Escape') els.picker.classList.add('hidden');
});
function startConversation(x) {
  els.picker.classList.add('hidden');
  const want = norm(x.address);
  const c = state.chats.find(ch => !ch.is_group && ch.participants.length === 1 && norm(ch.participants[0].address) === want);
  if (c) { openChat(c.id); return; }
  state.newChat = { address: x.address, name: x.raw ? x.address : x.name }; state.currentId = null;
  els.app.classList.add('show-chat'); renderChats(); renderHeader(); els.msgList.innerHTML = ''; els.loadMore.classList.add('hidden'); els.input.focus();
}

// ---------- keyboard ----------
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === 'n') { e.preventDefault(); els.newBtn.click(); }
  if ((e.metaKey || e.ctrlKey) && e.key === 'f' && !e.shiftKey) { e.preventDefault(); els.search.focus(); els.search.select(); }
  if (e.key === 'Escape') { els.emojiPanel.classList.add('hidden'); els.lightbox.classList.add('hidden'); }
});
document.addEventListener('visibilitychange', () => { if (!document.hidden && state.currentId) renderMessages(false); });

// ---------- start ----------
// A link of the form http://host:8787/#token=... (printed by the installer) logs in
// without typing. The fragment never reaches the server; drop it from the URL once read.
{
  const m = /(?:^#|[#&])token=([^&]+)/.exec(location.hash || '');
  if (m) {
    state.token = decodeURIComponent(m[1]);
    localStorage.setItem('imsg_token', state.token);
    document.cookie = `imsg_token=${encodeURIComponent(state.token)}; path=/; SameSite=Lax; max-age=31536000`;
    history.replaceState(null, '', location.pathname + location.search);
  }
}
if (state.token) boot().catch(() => showLogin()); else showLogin();
// If the stored token is rejected, api() shows the login overlay.
})();
