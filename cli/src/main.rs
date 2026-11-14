use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::{Parser, Subcommand};
use qrcode::{render::unicode, QrCode};

const APP: &str = "https://jetdafih.github.io/yoink/app";

const ART: &str = r#"                            ####
                    ########  ##
            ########          ##
    ########                  ##
  ##                      ####
    ####              ####==##
        ####      ####======##
            ######========##
            ##==========##
              ##======##
                ##==##
                  ##"#;

/// one keypress: y or Enter replaces, anything else keeps the existing file
fn confirm(p: &Path) -> bool {
    use crossterm::{
        event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
        terminal,
    };
    if !std::io::stdin().is_terminal() {
        eprintln!("{} exists, skipped", p.display());
        return false;
    }
    eprint!("{} exists. Replace? [Y/n] ", p.display());
    if terminal::enable_raw_mode().is_err() {
        return false;
    }
    let mut errors = 0;
    let yes = loop {
        let ev = match event::read() {
            Ok(ev) => ev,
            Err(_) if errors < 5 => {
                errors += 1;
                continue;
            }
            // don't get stuck in raw mode if the terminal stops answering
            Err(_) => break false,
        };
        let Event::Key(k) = ev else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            let _ = terminal::disable_raw_mode();
            eprintln!();
            std::process::exit(130);
        }
        break matches!(k.code, KeyCode::Enter | KeyCode::Char('y' | 'Y'));
    };
    let _ = terminal::disable_raw_mode();
    eprintln!("{}", if yes { "y" } else { "n" });
    yes
}

/// ctrl-c is caught so the share can shut down cleanly; the terminal still
/// echoes `^C`, so end the line for the shell prompt that follows
fn interrupted() {
    if std::io::stderr().is_terminal() {
        eprintln!();
    }
}

#[derive(Parser)]
#[command(
    name = "yoink",
    version,
    before_help = ART,
    arg_required_else_help = true,
    flatten_help = true,
    disable_version_flag = true
)]
struct Cli {
    /// print version
    #[arg(short = 'v', long, action = clap::ArgAction::Version)]
    version: Option<bool>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Send {
        /// stop once one receiver has finished downloading
        #[arg(long)]
        one: bool,
        /// show the ticket as a QR code
        #[arg(long)]
        qr: bool,
        /// print the bare ticket and a link that opens it in the web app
        #[arg(long)]
        url: bool,
        /// files and folders to send; more than one is bundled into an archive
        paths: Vec<PathBuf>,
    },
    Get {
        /// ticket printed by `yoink send`
        ticket: yoink_core::Ticket,
        /// folder to write the received files into
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Send {
            one,
            qr,
            url,
            paths,
        } => {
            let share = yoink_core::share(&paths).await?;
            if qr {
                // uppercase lets the encoder use the denser alphanumeric mode; Ticket parsing is case-insensitive
                let code = QrCode::new(share.ticket.to_string().to_uppercase())?;
                println!(
                    "{}",
                    code.render::<unicode::Dense1x2>()
                        .dark_color(unicode::Dense1x2::Light)
                        .light_color(unicode::Dense1x2::Dark)
                        .build()
                );
            }
            if url {
                println!("{}\n{APP}#{}", share.ticket, share.ticket);
            } else if !qr {
                println!("yoink get {}", share.ticket);
            }
            if one {
                tokio::select! {
                    _ = share.finished() => {}
                    _ = tokio::signal::ctrl_c() => interrupted(),
                }
            } else {
                let _ = tokio::signal::ctrl_c().await;
                interrupted();
            }
            share.close().await?;
        }
        Cmd::Get { ticket, out } => {
            for f in yoink_core::get(&ticket, &out, &mut confirm).await? {
                println!("{}", f.display());
            }
        }
    }
    Ok(())
}
