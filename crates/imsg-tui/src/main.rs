mod api;
mod app;
mod config;
mod ui;

use anyhow::Result;
use app::{App, AppEvent};
use clap::Parser;
use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(name = "imsg", version, about = "iMessage in your terminal")]
struct Args {
    /// Server URL, e.g. http://my-mac:8787 (default from ~/.config/imsg/client.toml or localhost)
    #[arg(long, short, env = "IMSG_SERVER")]
    server: Option<String>,
    /// Auth token (default from client.toml, or server.toml on the same Mac)
    #[arg(long, short, env = "IMSG_TOKEN")]
    token: Option<String>,
    /// Save --server/--token to ~/.config/imsg/client.toml and exit
    #[arg(long)]
    save: bool,
    /// Send a message from the command line and exit: --to <address|chat guid> --text "..."
    #[arg(long)]
    to: Option<String>,
    #[arg(long)]
    text: Option<String>,
    /// Attach a file when sending from the command line
    #[arg(long)]
    file: Vec<String>,
    /// Print the conversation list as JSON and exit
    #[arg(long)]
    list: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = config::load(args.server.clone(), args.token.clone())?;
    if args.save {
        config::save(&cfg)?;
        println!("saved to ~/.config/imsg/client.toml");
        return Ok(());
    }
    let api = api::Api::new(cfg);
    if args.list {
        let chats = api.chats().await?;
        println!("{}", serde_json::to_string_pretty(&chats)?);
        return Ok(());
    }
    if let Some(to) = &args.to {
        let mut req = imsg_core::SendRequest { text: args.text.clone().map(|t| app::expand_shortcodes(&t)), ..Default::default() };
        if to.contains(';') { req.chat_guid = Some(to.clone()); } else { req.to = vec![to.clone()]; }
        for f in &args.file {
            let p = app::expand_tilde(f);
            let up = api.upload(&p).await?;
            req.uploads.push(up.id);
        }
        let r = api.send(&req).await?;
        if r.ok { println!("sent"); } else { eprintln!("failed: {}", r.detail.unwrap_or_default()); std::process::exit(1); }
        return Ok(());
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut app = App::new(api.clone(), tx.clone());
    app.load_info();
    app.load_chats();
    app.load_contacts();
    {
        let api = api.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let (wtx, mut wrx) = mpsc::unbounded_channel();
            let a = api.clone();
            tokio::spawn(async move { a.ws_loop(wtx).await });
            while let Some(ev) = wrx.recv().await {
                if tx.send(AppEvent::Ws(ev)).is_err() { break; }
            }
        });
    }

    let mut terminal = ratatui::init();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    let result: Result<()> = loop {
        if app.force_redraw {
            app.force_redraw = false;
            let _ = terminal.clear();
        }
        if let Err(e) = terminal.draw(|f| ui::draw(f, &mut app)) {
            break Err(e.into());
        }
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) => { if let Err(e) = app.on_key(Event::Key(k)) { app.set_status(format!("{e}")); } }
                Some(Ok(Event::Paste(s))) => { app.compose.insert_str(&s); }
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(()),
            },
            Some(ev) = rx.recv() => { app.on_app_event(ev); }
            _ = tick.tick() => {}
        }
        if app.should_quit {
            break Ok(());
        }
    };
    ratatui::restore();
    result
}
