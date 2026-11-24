use std::{io::ErrorKind, process::Command};

use anyhow::{Result, bail};

pub const WIDTH: usize = 90;

pub struct Manual {
    pub original: String,
    pub blocks: Vec<Block>,
}

pub struct Block {
    pub original: String,
    /// Byte offset where the text to translate begins, if any.
    pub prose: Option<usize>,
}

pub fn read(section: Option<&str>, page: &str) -> Result<Manual> {
    let mut command = Command::new("man");
    command
        .env("MANPAGER", "cat")
        .env("PAGER", "cat")
        .env("MANWIDTH", WIDTH.to_string())
        .env("LC_ALL", "C")
        .env_remove("MANOPT")
        .env_remove("MAN_KEEP_FORMATTING");
    if let Some(section) = section {
        command.arg(section);
    }
    let output = command.arg(page).output().map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            anyhow::anyhow!("install the system man command and manual pages first")
        } else {
            error.into()
        }
    })?;
    if !output.status.success() {
        bail!(
            "could not open man page {page}: {}",
            plain_text(&String::from_utf8_lossy(&output.stderr)).trim()
        );
    }
    let original = plain_text(&String::from_utf8_lossy(&output.stdout));
    if original.trim().is_empty() {
        bail!("man returned an empty page for {page}");
    }
    let blocks = parse(&original);
    Ok(Manual { original, blocks })
}

fn plain_text(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars();
    while let Some(character) = chars.next() {
        match character {
            '\x08' => {
                if !output.ends_with('\n') {
                    output.pop();
                }
            }
            '\x1b' => match chars.next() {
                Some('[') => {
                    for character in chars.by_ref() {
                        if ('@'..='~').contains(&character) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    let mut escape = false;
                    for character in chars.by_ref() {
                        if character == '\x07' || (escape && character == '\\') {
                            break;
                        }
                        escape = character == '\x1b';
                    }
                }
                _ => {}
            },
            '\n' | '\t' => output.push(character),
            character if !character.is_control() => output.push(character),
            _ => {}
        }
    }
    output
}

fn parse(text: &str) -> Vec<Block> {
    let lines: Vec<_> = text.lines().collect();
    let first = lines.iter().position(|line| !line.trim().is_empty());
    let last = lines.iter().rposition(|line| !line.trim().is_empty());
    let mut section = "";
    let mut exit_status_indent = None;
    let mut blocks = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            push(&mut blocks, "", None);
            continue;
        }
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        let is_margin = Some(index) == first || Some(index) == last;
        if indent == 0 && !is_margin {
            section = trimmed;
        }
        let option = trimmed.starts_with('-');
        let columns = trimmed.split_once("  ");
        let numeric_row =
            columns.is_some_and(|(label, _)| label.bytes().all(|byte| byte.is_ascii_digit()));
        let exit_status_heading = trimmed
            .trim_end_matches(':')
            .eq_ignore_ascii_case("exit status");
        if exit_status_heading {
            exit_status_indent = Some(indent);
        } else if exit_status_indent.is_some_and(|heading_indent| {
            indent <= heading_indent && !(indent == heading_indent && numeric_row)
        }) {
            exit_status_indent = None;
        }
        let inline_description = columns
            .filter(|_| option || (exit_status_indent.is_some() && numeric_row))
            .map(|(label, rest)| indent + label.len() + 2 + rest.len() - rest.trim_start().len());
        let protocol = (section == "PROTOCOLS" && indent > 0)
            .then(|| protocol_label(trimmed))
            .flatten();
        let prose = if is_margin
            || matches!(section, "SYNOPSIS" | "EXAMPLE" | "EXAMPLES" | "SEE ALSO")
            || trimmed.starts_with("$ ")
        {
            None
        } else if let Some(label) = protocol {
            let description = trimmed[label.len()..].trim_start();
            (!description.is_empty()).then(|| indent + trimmed.len() - description.len())
        } else if let Some(offset) = inline_description {
            Some(offset)
        } else if option {
            None
        } else if section == "NAME" && trimmed.contains(" - ") {
            trimmed.find(" - ").map(|position| indent + position + 3)
        } else {
            Some(indent)
        };
        if prose == Some(indent)
            && indent > 0
            && let Some(previous) = blocks.last_mut()
            && previous.prose == Some(indent)
        {
            previous.original.push('\n');
            previous.original.push_str(line);
        } else {
            push(&mut blocks, line, prose);
        }
    }
    blocks
}

fn protocol_label(line: &str) -> Option<&str> {
    let label = line.split_whitespace().next()?;
    let name = label.strip_suffix("(S)").unwrap_or(label);
    let valid = name.starts_with(|character: char| character.is_ascii_uppercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
    valid.then_some(label)
}

fn push(blocks: &mut Vec<Block>, line: &str, prose: Option<usize>) {
    blocks.push(Block {
        original: line.to_owned(),
        prose,
    });
}

impl Block {
    pub fn source(&self) -> Option<String> {
        let start = self.prose?;
        let source = self.original[start..]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (!source.is_empty()).then_some(source)
    }

    pub fn translated(&self, text: &str) -> String {
        format!(
            "{}{}",
            &self.original[..self.prose.unwrap_or_default()],
            plain_text(text)
        )
    }

    pub fn translated_below(&self, text: &str) -> String {
        let indent = self.prose.unwrap_or_default();
        format!("{}{}", " ".repeat(indent), plain_text(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_protocol_labels_with_inline_or_separate_descriptions() {
        let raw = "CURL(1)\n\nPROTOCOLS\n       FTP(S) curl supports FTP.\n              With or without TLS.\n\n       HTTP(S)\n              curl supports HTTP.\n\nDESCRIPTION\n       TLS can be used.\nCURL(1)\n";
        let blocks = parse(raw);
        let sources: Vec<_> = blocks.iter().filter_map(Block::source).collect();
        assert_eq!(
            sources,
            [
                "PROTOCOLS",
                "curl supports FTP. With or without TLS.",
                "curl supports HTTP.",
                "DESCRIPTION",
                "TLS can be used."
            ]
        );
        let row = &blocks[3];
        assert_eq!(
            row.translated("curl 支持 FTP。"),
            "       FTP(S) curl 支持 FTP。"
        );
        assert_eq!(
            row.translated_below("curl 支持 FTP。"),
            "              curl 支持 FTP。"
        );
    }

    #[test]
    fn keeps_numeric_table_labels_out_of_translation() {
        let raw = "LS(1)\n\nDESCRIPTION\n       10  files can be listed.\n   Exit status:\n       0      if OK,\n       1      if minor problems,\n       2      if serious trouble.\n       12     another exit status.\n\nNOTES\n       20  files are needed.\nLS(1)\n";
        let blocks = parse(raw);
        let sources: Vec<_> = blocks.iter().filter_map(Block::source).collect();
        assert_eq!(
            sources,
            [
                "DESCRIPTION",
                "10 files can be listed.",
                "Exit status:",
                "if OK,",
                "if minor problems,",
                "if serious trouble.",
                "another exit status.",
                "NOTES",
                "20 files are needed."
            ]
        );
        let row = blocks
            .iter()
            .find(|block| block.original.contains("if minor problems"))
            .unwrap();
        assert_eq!(row.translated("出现轻微问题"), "       1      出现轻微问题");
        assert_eq!(
            row.translated_below("出现轻微问题"),
            "              出现轻微问题"
        );
    }

    #[test]
    fn preserves_option_labels_and_examples() {
        let raw = "DEMO(1)\n\nSYNOPSIS\n    demo [-q]\n\nOPTIONS\n    -q  Print less.\n        Useful in scripts.\n\nEXAMPLES\n    demo -q\n\nDEMO(1)\n";
        let blocks = parse(raw);
        let sources: Vec<_> = blocks.iter().filter_map(Block::source).collect();
        assert_eq!(sources, ["OPTIONS", "Print less. Useful in scripts."]);
        let option = blocks
            .iter()
            .find(|block| block.original.contains("Print less."))
            .unwrap();
        assert_eq!(option.translated("减少输出。"), "    -q  减少输出。");
        assert_eq!(plain_text("\x1b[1mB\x08B\x1b[0m _\x08a"), "B a");
    }
}
