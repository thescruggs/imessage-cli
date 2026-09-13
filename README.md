# imsg — your Mac's iMessages in a browser or terminal

imsg is a small program that runs on a Mac and lets you read and send your
iMessages from a web page or from a terminal window. Once it is running you
can also open that web page from another computer or a phone on the same
Wi‑Fi (or over Tailscale).

It only works on a Mac that is signed in to iMessage, because it reads the
Messages app's own database and sends through the Messages app. Nothing is
uploaded anywhere; everything stays on your Mac.

---

## Install (about 5 minutes)

You do not need to know how to program. You will paste one command into the
Terminal app and then click through two permission screens.

### What you need

- A Mac running macOS 13 (Ventura) or newer. Apple Silicon and Intel both work.
- The **Messages** app on that Mac signed in to iMessage (open Messages and make
  sure you can see your conversations).
- An internet connection for the download.

### Step 1 — Open Terminal

Press **Command + Space**, type `Terminal`, and press **Return**. A window with
a blinking cursor appears. Terminal shows a lot of text while things install;
that is normal.

### Step 2 — Paste the install command

Copy this whole line, paste it into Terminal, and press **Return**:

```sh
curl -fsSL https://github.com/thescruggs/imessage-cli/releases/latest/download/install.sh | sh
```

It downloads imsg, puts it in a folder inside your home folder
(`~/.local/bin`), and sets it to start automatically whenever you log in.
It never asks for your password and does not need `sudo`.

When it finishes it prints a **NEXT STEPS** box. Keep that Terminal window
open; you will need the link it printed in Step 4.

### Step 3 — Allow imsg to read your messages

macOS does not let programs read your messages until you say so. The installer
opens two windows for you:

- **System Settings**, on the *Full Disk Access* page.
- A **Finder** window with a file called `imsg-server` selected.

Do this:

1. Drag `imsg-server` from the Finder window into the list on the Full Disk
   Access page. (If dragging does not work: click the **+** button under the
   list, press **Command + Shift + G**, type `~/.local/bin/imsg-server`, press
   Return, then click **Open**.)
2. Make sure the switch next to **imsg-server** is turned **on**. macOS may ask
   for your Mac password or Touch ID.
3. If macOS asks whether to quit and reopen imsg-server, choose **Quit & Reopen**
   or **Later**; either is fine.

You do not need to restart anything. imsg retries by itself every few seconds
and will start working as soon as the switch is on.

### Step 4 — Open the web page

Look at the **NEXT STEPS** box in Terminal. It contains a link like

```text
http://localhost:8787/#token=Abc123...
```

Hold **Command** and click the link (or copy and paste it into Safari or
Chrome). Your conversations should appear within a few seconds. The long code
after `#token=` is your private login key; the page remembers it, so you only
need the link once.

If you lost the Terminal window, you can open the page any time with:

```sh
~/.local/bin/imsg-service --open
```

### Step 5 — Send your first message

Pick a conversation, type a message, press Return. The **first time** you send,
macOS shows a window asking whether **imsg-server** may control **Messages**.
Click **Allow** (or **OK**). If your Mac's firewall is on, macOS may also ask
whether imsg-server can accept incoming connections; click **Allow** so other
devices on your network can connect.

That's it. imsg now starts by itself every time you log in.

---

## Everyday use

### On the Mac

Open `http://localhost:8787` in any browser. If it asks for a token, run
`~/.local/bin/imsg-service --open` in Terminal instead, or paste the token from
`~/.local/bin/imsg-service --token`.

### From your phone or another computer

Both devices must be on the same Wi‑Fi network (or both on your Tailscale
network). On the Mac, run:

```sh
~/.local/bin/imsg-service --url
```

It prints the addresses to use, for example
`http://Aarons-MacBook.local:8787/#token=...` and a numeric one like
`http://192.168.1.25:8787/#token=...`. Open one of them on the other device. If
the `.local` name does not load, use the numeric one. Anyone with that link can
read and send your messages, so only share it with yourself.

The Mac must be awake and logged in for other devices to connect. In
**System Settings → Displays → Advanced** (or **Battery → Options** on a laptop)
turn on *Prevent automatic sleeping when the display is off* if you want it
reachable all the time.

### Web page basics

- Click a conversation on the left to open it. Unread ones are bold.
- Type at the bottom and press **Return** to send. **Shift + Return** adds a
  new line.
- Drag a photo or file onto the page, or click the **＋** button next to the message box, to send it.
- Reactions (tapbacks), replies, edited messages and read receipts from your
  phone are shown, and pictures and videos display inline.
- New messages appear live; the browser tab title shows the unread count.

### Helper commands

All of these are typed into Terminal:

| Command | What it does |
| --- | --- |
| `~/.local/bin/imsg-service --open` | Open the web page, already logged in |
| `~/.local/bin/imsg-service --url` | Show the links for this Mac and other devices |
| `~/.local/bin/imsg-service --token` | Show the login token |
| `~/.local/bin/imsg-service --status` | Check whether the server is running |
| `~/.local/bin/imsg-service --restart` | Restart the server |
| `~/.local/bin/imsg-service --update` | Download and install the newest version |
| `~/.local/bin/imsg-service --uninstall` | Stop it from running (see below) |

### Updating

```sh
~/.local/bin/imsg-service --update
```

After an update macOS forgets the Full Disk Access permission, because the
program file changed. If your messages do not load afterwards, open
**System Settings → Privacy & Security → Full Disk Access**, select
**imsg-server**, click **−** to remove it, then add it again exactly as in
Step 3.

### Uninstalling

```sh
~/.local/bin/imsg-service --uninstall
```

This stops the server and removes the "start at login" entry. To remove the
files too, delete `~/.local/bin/imsg`, `~/.local/bin/imsg-server`,
`~/.local/bin/imsg-service`, the folder `~/.config/imsg`, and the folder
`~/Pictures/imsg-uploads`. Then remove imsg-server from the Full Disk Access
list in System Settings.

---

## Troubleshooting

**The page opens but says it cannot load, or the conversation list is empty.**
Full Disk Access is missing. Go through Step 3 again and make sure the switch
next to imsg-server is on. Then wait ten seconds and reload the page. If it
still fails, run `~/.local/bin/imsg-service --restart`.

**The page asks for a token.**
Run `~/.local/bin/imsg-service --open` on the Mac, or paste the output of
`~/.local/bin/imsg-service --token` into the box.

**"Safari can't connect to the server" / nothing loads at all.**
Make sure you typed `http://` (not `https://`) and included `:8787`. Then run
`~/.local/bin/imsg-service --status`. If it says the service is not
registered, run the install command from Step 2 again.

**Sending does nothing, or a message shows a red error.**
Look for the "imsg-server wants to control Messages" window and click Allow.
If you clicked Don't Allow earlier, go to **System Settings → Privacy &
Security → Automation**, find **imsg-server**, and turn on **Messages**.

**Another device cannot connect.**
Check that both devices are on the same Wi‑Fi, try the numeric address from
`--url`, and make sure the Mac is awake. If the Mac's firewall is on and it
asked about imsg-server, choose Allow. If you installed with
`--bind 127.0.0.1:8787` the server only accepts this Mac.

**macOS says the program "cannot be opened because Apple cannot check it for
malicious software".**
This happens if the package was downloaded with a browser instead of the
install command. Use the command in Step 2, which avoids that flag.

**Something else.**
The server writes a log to `~/.config/imsg/server.log`. Open it with
`open ~/.config/imsg/server.log` and look at the last few lines.

---

## Installing by hand instead

If you would rather not paste a command that downloads and runs a script:

1. Download `imsg-macos-universal.tar.gz` from the
   [Releases page](https://github.com/thescruggs/imessage-cli/releases/latest)
   and double-click it to unpack.
2. In Terminal, run the installer inside the unpacked folder:

   ```sh
   sh ~/Downloads/imsg-0.1.0-macos/install-server.sh --open-settings
   ```

3. Continue from Step 3 above.

To allow connections **only from this Mac**, add `--bind 127.0.0.1:8787` to
that command, or set `IMSG_BIND=127.0.0.1:8787` in front of the one-line
installer.

---

## Terminal client

Besides the web page, imsg comes with a terminal client that shows the same
conversations as iMessage-style bubbles, with contact names, reactions,
replies, attachments, an emoji picker and file sending.

```sh
~/.local/bin/imsg
```

It finds the token automatically on the Mac that runs the server. To use it from
another computer that has the client, save the connection once:

```sh
imsg --server http://Aarons-MacBook.local:8787 --token '<token>' --save
imsg
```

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Move between the chat list, the messages, and the compose box |
| `j` / `k` or arrow keys | Move through chats or messages |
| `Enter` | Open the compose box, or send |
| `Alt+Enter`, `Ctrl+J` | New line while composing |
| `:smile:` | Emoji shortcode; `Ctrl+E` opens the emoji picker |
| `Ctrl+F` | Attach a file (with Tab completion) |
| `Ctrl+N` | New message to a contact, number, or email |
| `Ctrl+S` | Search all messages |
| `/` | Filter the chat list |
| `o` / `s` / `y` | Open, save, or copy the selected attachment or text |
| `Ctrl+R`, `Ctrl+L` | Refresh, redraw |
| `?` | Help |

One-shot commands without the UI:

```sh
~/.local/bin/imsg --list                                            # chats as JSON
~/.local/bin/imsg --to "+15551234567" --text "On my way :thumbsup:"
~/.local/bin/imsg --to "+15551234567" --file ~/Pictures/photo.jpg
```

---

## Privacy and security notes

- The server listens on all network interfaces by default so phones and other
  computers on your network can connect. Everything requires the token; without
  it, requests are refused. Treat the token (and any link containing it) like a
  password. To generate a new one, delete `~/.config/imsg/server.toml` and run
  `~/.local/bin/imsg-service --restart`.
- Traffic is plain HTTP. On your home Wi‑Fi or over Tailscale that is normally
  fine; do not expose port 8787 to the internet.
- Files you send are staged in `~/Pictures/imsg-uploads` (Messages can only
  read files from standard folders). You can delete that folder's contents at
  any time.

## What it cannot do

- Send tapback reactions, edit or unsend messages, or create *new* group chats.
  Apple does not expose those to other programs. Existing group chats work.
- Mark conversations as read on your phone. Read receipts stay with Messages.
- Show attachments that were never downloaded to this Mac.

---

## For developers

The project is a Rust workspace:

```text
crates/imsg-core     shared wire types
crates/imsg-server   axum HTTP + WebSocket server; reads ~/Library/Messages/chat.db,
                     Contacts, sends via Messages.app (AppleScript), HEIC previews via sips
crates/imsg-tui      the `imsg` terminal client (ratatui)
web/                 the browser client, embedded into imsg-server at build time
scripts/             install-server.sh (installer / imsg-service helper),
                     install.sh (curl bootstrap), package.sh (release build), tests
```

Build and run from source (needs [rustup](https://rustup.rs) and Xcode command
line tools):

```sh
cargo build --release
./target/release/imsg-server --bind 127.0.0.1:8787   # prints the token
./target/release/imsg
./scripts/install-server.sh                          # build + install as a LaunchAgent
```

Tests:

```sh
cargo test --workspace --locked
python3 scripts/test-install-server.py        # installer, fully mocked (no launchd)
deno test --allow-read=web/app.js web/app_test.js
```

Release package (universal Apple Silicon + Intel binaries in `dist/`):

```sh
./scripts/package.sh
gh release create vX.Y.Z dist/imsg-macos-universal.tar.gz dist/install.sh dist/SHA256SUMS
```

`install.sh` always fetches the asset named `imsg-macos-universal.tar.gz` from
the latest GitHub release, so keep that name when publishing.

### HTTP API

Every `/api` route and `/ws` requires the token: `Authorization: Bearer`, a
`?token=` query parameter, or the `imsg_token` cookie the web client sets.

| Route | Purpose |
| --- | --- |
| `GET /api/info` | Server name, version, host, your own addresses |
| `GET /api/chats?limit=` | Conversations, newest first, with participants, preview, unread |
| `GET /api/chats/{id}/messages?limit=&before=` | Messages oldest→newest; `before` pages back by id |
| `POST /api/chats/{id}/seen` | `{message_id}` (optional) marks the chat viewed up to that message; clears imsg's unread count and broadcasts `chats` |
| `GET /api/messages/{guid}` | One message |
| `GET /api/search?q=` | Search message text |
| `GET /api/contacts` | Contacts (name, phones, emails, has_photo) |
| `GET /api/contacts/{id}/photo` | Contact thumbnail |
| `GET /api/attachments/{id}[?preview=1][&download=1]` | Attachment bytes; preview converts HEIC and downsizes; supports Range |
| `POST /api/upload` (multipart `file`) | Stage a file for sending; returns an upload id |
| `POST /api/send` | `{chat_guid}` or `{to:[address]}` plus `text`, `uploads[]`, `paths[]` |
| `GET /ws?token=` | WebSocket: `hello`, `messages` (new/changed), `chats` events |

Messages carry sender (with contact name), styled text parts, attachments,
tapbacks, reply quotes, edited/unsent state, expressive effects,
delivery/read timestamps, and app/link balloons.

## License

This is free and unencumbered software released into the public domain under
[The Unlicense](LICENSE). Do whatever you want with it; no attribution required.
