use std::{
    io::{self, Write},
    sync::mpsc::TryRecvError,
    time::Duration,
};

use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Color, Print, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::{
    cli::{self, Mode},
    manual::Manual,
    settings::{Settings, Storage},
    translation::{Job, Update},
};

const TRANSLATION_COLOR: Color = Color::AnsiValue(180);

#[derive(Default)]
enum Segment {
    #[default]
    Pending,
    Ready(String),
    Failed,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Position {
    block: usize,
    line: usize,
}

struct DisplayLine {
    position: Position,
    text: String,
    highlight_translation: bool,
}

pub fn read(manual: &Manual, mode: Mode, storage: Option<&Storage>) -> Result<()> {
    let loaded = storage.map(Storage::load).transpose();
    let (mut settings, mut message) = match loaded {
        Ok(settings) => (settings.unwrap_or_default(), None),
        Err(error) => (Settings::default(), Some(format!("{error:#}"))),
    };
    let mut position = Position::default();

    loop {
        let action = {
            let _terminal = Terminal::enter()?;
            screen(
                manual,
                mode,
                storage,
                &mut settings,
                &mut position,
                message.take(),
            )?
        };
        match action {
            Action::Quit => return Ok(()),
            Action::Configure => {
                if let Some(storage) = storage {
                    match cli::configure(storage) {
                        Ok(()) => settings = storage.load()?,
                        Err(error) => {
                            eprintln!("mangl: {error:#}\nPress Enter to return to the manual.");
                            let _ = io::stdin().read_line(&mut String::new());
                            message = Some(error.to_string());
                        }
                    }
                }
            }
        }
    }
}

enum Action {
    Quit,
    Configure,
}

struct Terminal;

impl Terminal {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let session = Self;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(session)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn screen(
    manual: &Manual,
    mode: Mode,
    storage: Option<&Storage>,
    settings: &mut Settings,
    position: &mut Position,
    mut message: Option<String>,
) -> Result<Action> {
    let mut results: Vec<_> = manual.blocks.iter().map(|_| Segment::Pending).collect();
    let mut job = start(manual, settings, storage, position.block)?;
    let mut dirty = true;
    let mut top = 0;
    let mut lines = Vec::new();
    let mut height = 1;

    loop {
        if let Some(active) = &job {
            loop {
                match active.updates.try_recv() {
                    Ok(Update::Segment(id, result)) => {
                        results[id] = result.map_or(Segment::Failed, Segment::Ready);
                        dirty = true;
                    }
                    Ok(Update::Error(error)) => {
                        message = Some(error);
                        dirty = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        job = None;
                        for segment in &mut results {
                            if matches!(segment, Segment::Pending) {
                                *segment = Segment::Failed;
                            }
                        }
                        dirty = true;
                        break;
                    }
                }
            }
        }

        if dirty {
            let (width, rows) = terminal::size()?;
            height = usize::from(rows.saturating_sub(2)).max(1);
            let width = usize::from(width).max(1);
            lines = render_lines(manual, mode, &results, width);
            let anchor = lines.partition_point(|line| {
                let line = line.position;
                (line.block, line.line) < (position.block, position.line)
            });
            top = anchor.min(lines.len().saturating_sub(height));
            let status = message.as_deref().unwrap_or_else(|| {
                if job.is_some() {
                    "Translating…"
                } else if storage.is_some()
                    && settings.translation.is_none()
                    && !settings.skipped_setup
                {
                    "Translation is not configured. c: configure · s: later"
                } else if storage.is_some() && settings.translation.is_none() {
                    "Translation is not configured. c: configure"
                } else if manual.blocks.iter().zip(&results).any(|(block, result)| {
                    block.prose.is_some() && matches!(result, Segment::Failed)
                }) {
                    "Some translations are unavailable. r: retry"
                } else {
                    ""
                }
            });
            let help = if storage.is_some() {
                "↑↓/jk scroll · PgUp/PgDn · c configure · r retry · q quit"
            } else {
                "↑↓/jk scroll · PgUp/PgDn · q quit"
            };
            let mut stdout = io::stdout().lock();
            queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;
            for (row, line) in lines
                .iter()
                .skip(top)
                .take(height.min(usize::from(rows)))
                .enumerate()
            {
                queue!(stdout, MoveTo(0, row as u16))?;
                if line.highlight_translation {
                    queue!(
                        stdout,
                        SetForegroundColor(TRANSLATION_COLOR),
                        Print(&line.text),
                        SetForegroundColor(Color::Reset)
                    )?;
                } else {
                    queue!(stdout, Print(&line.text))?;
                }
            }
            if rows >= 2 {
                queue!(
                    stdout,
                    MoveTo(0, rows - 2),
                    Print(first_line(status, width))
                )?;
            }
            if rows >= 1 {
                queue!(stdout, MoveTo(0, rows - 1), Print(first_line(help, width)))?;
            }
            stdout.flush()?;
            dirty = false;
        }

        if !event::poll(Duration::from_millis(75))? {
            continue;
        }
        let key = match event::read()? {
            Event::Resize(..) => {
                dirty = true;
                continue;
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => key,
            _ => continue,
        };
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(Action::Quit);
        }
        let next = match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(Action::Quit),
            KeyCode::Char('c') if storage.is_some() => return Ok(Action::Configure),
            KeyCode::Char('s') if settings.translation.is_none() && !settings.skipped_setup => {
                if let Some(storage) = storage {
                    settings.skipped_setup = true;
                    if let Err(error) = storage.save(settings) {
                        message = Some(format!("{error:#}"));
                    }
                    dirty = true;
                }
                continue;
            }
            KeyCode::Char('r') if job.is_none() && settings.translation.is_some() => {
                for segment in &mut results {
                    if matches!(segment, Segment::Failed) {
                        *segment = Segment::Pending;
                    }
                }
                message = None;
                job = start(manual, settings, storage, position.block)?;
                dirty = true;
                continue;
            }
            KeyCode::Down | KeyCode::Char('j') => top.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => top.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => top.saturating_add(height),
            KeyCode::PageUp => top.saturating_sub(height),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => lines.len(),
            _ => continue,
        };
        if let Some(line) = lines.get(next.min(lines.len().saturating_sub(height))) {
            *position = line.position;
            if let Some(job) = &job {
                job.focus(position.block);
            }
            dirty = true;
        }
    }
}

fn first_line(text: &str, width: usize) -> String {
    textwrap::wrap(text, width)
        .first()
        .map_or_else(String::new, ToString::to_string)
}

fn start(
    manual: &Manual,
    settings: &Settings,
    storage: Option<&Storage>,
    focus: usize,
) -> Result<Option<Job>> {
    let Some((storage, config)) = storage.zip(settings.translation.clone()) else {
        return Ok(None);
    };
    let job = Job::start(manual, config, storage.cache.clone(), focus)?;
    Ok(Some(job))
}

fn render_lines(
    manual: &Manual,
    mode: Mode,
    results: &[Segment],
    width: usize,
) -> Vec<DisplayLine> {
    let mut lines = Vec::new();
    for (id, (block, result)) in manual.blocks.iter().zip(results).enumerate() {
        let mut line_number = 0;
        let mut append = |text: &str, highlight_translation| {
            for source in text.split('\n') {
                let expanded = source.replace('\t', "    ");
                let indent = expanded.len() - expanded.trim_start_matches(' ').len();
                let indent = indent.min(width / 2);
                for line in textwrap::wrap(expanded.trim_start_matches(' '), width - indent) {
                    lines.push(DisplayLine {
                        position: Position {
                            block: id,
                            line: line_number,
                        },
                        text: format!("{}{line}", " ".repeat(indent)),
                        highlight_translation,
                    });
                    line_number += 1;
                }
            }
        };
        let translatable = block.prose.is_some();
        if mode != Mode::Translated || !translatable {
            append(&block.original, false);
        }
        if translatable {
            match result {
                Segment::Ready(text) if mode == Mode::Bilingual => {
                    append(&block.translated_below(text), true);
                }
                Segment::Ready(text) => append(&block.translated(text), false),
                Segment::Failed if mode == Mode::Translated => {
                    append(&block.translated("[translation unavailable]"), false)
                }
                Segment::Pending if mode == Mode::Translated => {
                    append(&block.translated("[awaiting translation]"), false)
                }
                _ => {}
            }
        }
    }
    lines
}
