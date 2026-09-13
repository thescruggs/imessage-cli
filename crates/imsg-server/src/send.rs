//! Send messages and files through Messages.app via AppleScript.
use anyhow::{anyhow, Result};
use tokio::process::Command;

const SCRIPT: &str = r#"
on run argv
    set target to item 1 of argv
    set mode to item 2 of argv
    set payload to item 3 of argv
    set what to item 4 of argv
    tell application "Messages"
        if target is "chat" then
            set dest to chat id mode
        else
            set dest to participant mode
        end if
        if what is "text" then
            send payload to dest
        else
            set f to POSIX file payload as alias
            send f to dest
        end if
    end tell
end run
"#;

async fn run(target: &str, dest: &str, payload: &str, what: &str) -> Result<()> {
    let out = Command::new("osascript")
        .arg("-")
        .arg(target)
        .arg(dest)
        .arg(payload)
        .arg(what)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use tokio::io::AsyncWriteExt;
            let mut stdin = child.stdin.take().unwrap();
            let script = SCRIPT.to_string();
            tokio::spawn(async move {
                let _ = stdin.write_all(script.as_bytes()).await;
            });
            Ok(child)
        })?
        .wait_with_output()
        .await?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(anyhow!("osascript failed: {err}"))
    }
}

/// AppleScript chat ids use the `any;` service prefix regardless of what the DB stored.
pub fn applescript_chat_id(guid: &str) -> String {
    match guid.find(';') {
        Some(i) => format!("any{}", &guid[i..]),
        None => guid.to_string(),
    }
}

pub async fn send_text_to_chat(chat_guid: &str, text: &str) -> Result<()> {
    run("chat", &applescript_chat_id(chat_guid), text, "text").await
}

pub async fn send_file_to_chat(chat_guid: &str, path: &str) -> Result<()> {
    run("chat", &applescript_chat_id(chat_guid), path, "file").await
}

pub async fn send_text_to_address(address: &str, text: &str) -> Result<()> {
    run("participant", address, text, "text").await
}

pub async fn send_file_to_address(address: &str, path: &str) -> Result<()> {
    run("participant", address, path, "file").await
}
