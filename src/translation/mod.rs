mod aliyun;

use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
};

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::{
    manual::Manual,
    settings::{self, Credentials, Provider, TranslationSettings},
};

pub use aliyun::{CONSOLE, DEFAULT_LANGUAGE};

pub enum Update {
    Segment(usize, Option<String>),
    Error(String),
}

pub struct Job {
    pub updates: Receiver<Update>,
    focus: Arc<AtomicUsize>,
}

impl Job {
    pub fn start(
        manual: &Manual,
        config: TranslationSettings,
        cache: PathBuf,
        initial_focus: usize,
    ) -> Result<Self> {
        let segments: Vec<_> = manual
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(id, block)| block.source().map(|source| (id, source)))
            .collect();
        let focus = Arc::new(AtomicUsize::new(initial_focus));
        let worker_focus = Arc::clone(&focus);
        let (sender, updates) = mpsc::channel();
        thread::Builder::new()
            .name("translation".into())
            .spawn(move || {
                let result = translate(segments, &config, &cache, &worker_focus, |update| {
                    sender.send(update).is_ok()
                });
                if let Err(error) = result {
                    let _ = sender.send(Update::Error(format!("{error:#}")));
                }
            })?;
        Ok(Self { updates, focus })
    }

    pub fn focus(&self, block: usize) {
        self.focus.store(block, Ordering::Relaxed);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.focus.store(usize::MAX, Ordering::Relaxed);
    }
}

pub fn validate(target: &str, credentials: Credentials) -> Result<()> {
    aliyun::Aliyun::new(credentials)?.validate(target)
}

struct Pending {
    id: usize,
    path: PathBuf,
    remaining: VecDeque<String>,
    translated: Vec<String>,
    failed: bool,
}

fn translate(
    segments: Vec<(usize, String)>,
    config: &TranslationSettings,
    cache: &Path,
    focus: &AtomicUsize,
    emit: impl Fn(Update) -> bool,
) -> Result<()> {
    let mut pending = Vec::new();
    for (id, source) in segments {
        if focus.load(Ordering::Relaxed) == usize::MAX {
            return Ok(());
        }
        // The system manual is read in the C locale. English needs no external call.
        if config.target_language == "en" {
            if !emit(Update::Segment(id, Some(source))) {
                return Ok(());
            }
            continue;
        }
        // Prefer the verified translation of the GNU ls option over a cached machine translation.
        if config.target_language == "zh" && source == "likewise, but use powers of 1000 not 1024" {
            if !emit(Update::Segment(
                id,
                Some("与 -h 类似，但以 1000 为基数换算大小，而不是 1024。".into()),
            )) {
                return Ok(());
            }
            continue;
        }
        let provider = match config.provider {
            Provider::Aliyun => "aliyun-general",
        };
        let hash = Sha256::digest(format!(
            "1\0{provider}\0en\0{}\0{source}",
            config.target_language
        ));
        let path = cache.join(format!("{hash:x}"));
        if let Ok(text) = fs::read_to_string(&path) {
            if !emit(Update::Segment(id, Some(text))) {
                return Ok(());
            }
        } else {
            pending.push(Pending {
                id,
                path,
                remaining: aliyun::chunk(&source)
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                translated: Vec::new(),
                failed: false,
            });
        }
    }
    if pending.is_empty() {
        return Ok(());
    }

    let client = aliyun::Aliyun::new(settings::credentials(config)?)?;
    while !pending.is_empty() {
        let current = focus.load(Ordering::Relaxed);
        if current == usize::MAX {
            break;
        }
        pending.sort_by_key(|segment| segment.id.abs_diff(current));
        let chunks = pending.iter().enumerate().flat_map(|(index, segment)| {
            segment
                .remaining
                .iter()
                .map(move |text| (index, text.as_str()))
        });
        let batch_len = aliyun::batch_size(chunks.clone().map(|(_, text)| text));
        let (owners, texts): (Vec<_>, Vec<_>) = chunks.take(batch_len).unzip();
        let results = client.translate("en", &config.target_language, &texts)?;
        for (owner, result) in owners.into_iter().zip(results) {
            let segment = &mut pending[owner];
            segment.remaining.pop_front();
            match result {
                Some(text) => segment.translated.push(text),
                None => segment.failed = true,
            }
        }

        for segment in pending.extract_if(.., |segment| segment.remaining.is_empty()) {
            let text = (!segment.failed).then(|| segment.translated.join(" "));
            if let Some(text) = &text {
                let _ = settings::write_atomic(&segment.path, text.as_bytes());
            }
            if !emit(Update::Segment(segment.id, text)) {
                return Ok(());
            }
        }
    }
    Ok(())
}
