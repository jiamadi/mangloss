use std::io::{self, IsTerminal, Write};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::{
    manual,
    settings::{Credentials, Storage},
    translation::{self, Job, Update},
    tui,
};

#[derive(Parser)]
#[command(
    name = "mangl",
    version,
    about = "Read local man pages with optional machine translation",
    override_usage = "mangl [-o | -t | -b] [SECTION] PAGE\n       mangl config",
    args_conflicts_with_subcommands = true,
    arg_required_else_help = true
)]
struct Args {
    #[arg(short = 'o', long, conflicts_with_all = ["translated", "bilingual"], help = "Show the original without making translation requests")]
    original: bool,
    #[arg(
        short = 't',
        long,
        conflicts_with = "bilingual",
        help = "Show only translated prose"
    )]
    translated: bool,
    #[arg(
        short = 'b',
        long,
        help = "Show the original and translation (default)"
    )]
    bilingual: bool,
    #[arg(value_name = "SECTION or PAGE")]
    first: Option<String>,
    #[arg(value_name = "PAGE", requires = "first")]
    page: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Set up Aliyun Machine Translation and the target language.
    Config,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Original,
    Translated,
    Bilingual,
}

pub fn run() -> Result<()> {
    let args = Args::parse();
    if let Some(Command::Config) = args.command {
        return configure(&Storage::new()?);
    }

    let first = args.first.as_deref().context("specify a manual page")?;
    let (section, page) = match args.page.as_deref() {
        Option::Some(page) => (Some(first), page),
        Option::None => (None, first),
    };
    if page.is_empty()
        || page.starts_with('-')
        || section.is_some_and(|s| s.is_empty() || s.starts_with('-'))
    {
        bail!("invalid manual page or section");
    }
    let mode = if args.original {
        Mode::Original
    } else if args.translated {
        Mode::Translated
    } else {
        Mode::Bilingual
    };
    let manual = manual::read(section, page)?;

    if mode == Mode::Original {
        if interactive() {
            return tui::read(&manual, mode, None);
        }
        return print_text(&manual.original);
    }

    let storage = Storage::new()?;
    if interactive() {
        tui::read(&manual, mode, Some(&storage))
    } else {
        let settings = storage.load()?;
        let Some(config) = settings.translation else {
            if mode == Mode::Translated {
                bail!("run mangl config to enable translation");
            }
            eprintln!("mangl: run mangl config to enable translation");
            return print_text(&manual.original);
        };
        let job = Job::start(&manual, config, storage.cache.clone(), 0)?;
        let mut results = vec![None; manual.blocks.len()];
        while let Ok(update) = job.updates.recv() {
            match update {
                Update::Segment(id, text) => results[id] = text,
                Update::Error(error) => eprintln!("mangl: {error}"),
            }
        }
        let mut output = String::new();
        let mut failed = false;
        for (block, translated) in manual.blocks.iter().zip(results) {
            let translatable = block.prose.is_some();
            if mode == Mode::Bilingual || !translatable {
                output.push_str(&block.original);
                output.push('\n');
            }
            if let Some(text) = translated {
                let line = if mode == Mode::Bilingual {
                    block.translated_below(&text)
                } else {
                    block.translated(&text)
                };
                output.push_str(&line);
                output.push('\n');
            } else if translatable && mode == Mode::Translated {
                failed = true;
                output.push_str(&block.translated("[translation unavailable]"));
                output.push('\n');
            }
        }
        print_text(&output)?;
        if failed {
            bail!("some segments could not be translated");
        }
        Ok(())
    }
}

fn print_text(text: &str) -> Result<()> {
    match io::stdout().lock().write_all(text.as_bytes()) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
}

fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

pub fn configure(storage: &Storage) -> Result<()> {
    if !interactive() {
        bail!("mangl config requires an interactive terminal");
    }

    let settings = storage.load()?;
    println!("Provider: Aliyun Machine Translation (General)");
    if prompt("Open the Aliyun console to activate the service? [y/N] ")?.eq_ignore_ascii_case("y")
    {
        open::that_detached(translation::CONSOLE)
            .unwrap_or_else(|_| println!("Open {}", translation::CONSOLE));
    }
    let default = settings
        .translation
        .as_ref()
        .map_or(translation::DEFAULT_LANGUAGE, |config| {
            &config.target_language
        });
    println!(
        "Target language examples: zh (Simplified Chinese), zh-tw (Traditional Chinese), ja, ko, fr, de, es."
    );
    let input = prompt(&format!("Target language [{default}]: "))?;
    let target = if input.is_empty() {
        default.to_owned()
    } else {
        input.to_ascii_lowercase()
    };
    if !target
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        bail!("enter an Aliyun language code such as zh or ja");
    }

    let replace = settings.translation.is_none()
        || prompt("Replace the stored AccessKey pair? [y/N] ")?.eq_ignore_ascii_case("y");
    let credentials = if replace {
        Credentials {
            access_key_id: rpassword::prompt_password("AccessKey ID (hidden): ")?
                .trim()
                .to_owned(),
            access_key_secret: rpassword::prompt_password("AccessKey Secret (hidden): ")?
                .trim()
                .to_owned(),
        }
    } else {
        crate::settings::credentials(
            settings
                .translation
                .as_ref()
                .context("no saved configuration")?,
        )?
    };
    if credentials.access_key_id.is_empty() || credentials.access_key_secret.is_empty() {
        bail!("both AccessKey fields are required");
    }
    println!("Verifying with one short translation request...");
    translation::validate(&target, credentials.clone())?;
    storage.configure(settings, target, &credentials)?;
    println!("Translation settings saved.");
    Ok(())
}

fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    io::stdout().flush()?;
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer)? == 0 {
        bail!("configuration cancelled");
    }
    Ok(answer.trim().to_owned())
}
