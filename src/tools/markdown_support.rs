use anyhow::{Context, Result};
use regex::Regex;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::tools::read_file::{count_text_lines, decode_fuzzy};

const MAX_MARKDOWN_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct MarkdownHeading {
    pub title: String,
    pub level: usize,
    pub line: usize,
    pub path: Vec<String>,
    pub slug: String,
}

#[derive(Clone, Copy, Debug)]
pub enum SectionSpanMode {
    IncludeSubsections,
    HeadingBodyOnly,
}

pub fn load_markdown_file(path: &Path) -> Result<(String, usize)> {
    if !path.exists() || !path.is_file() {
        return Err(anyhow::anyhow!(
            "Markdown file does not exist or is not a file: {}",
            path.display()
        ));
    }

    let meta = std::fs::metadata(path)?;
    if meta.len() > MAX_MARKDOWN_BYTES {
        return Err(anyhow::anyhow!(
            "Markdown file is too large ({} bytes > {} bytes)",
            meta.len(),
            MAX_MARKDOWN_BYTES
        ));
    }

    let mut file = File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let (content, _) = decode_fuzzy(&buffer);
    let total_lines = count_text_lines(&content);
    Ok((content, total_lines))
}

pub fn parse_headings(content: &str) -> Vec<MarkdownHeading> {
    let heading_re =
        Regex::new(r"^(#{1,6})[ \t]+(.+?)(?:[ \t]+#+[ \t]*)?$").expect("valid heading regex");
    let fence_re = Regex::new(r"^([`~]{3,})").expect("valid fence regex");

    let mut headings = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut active_fence: Option<(char, usize)> = None;

    for (index, raw_line) in content.split('\n').enumerate() {
        let line = raw_line.trim_end_matches('\r');
        let leading_spaces = line.chars().take_while(|ch| *ch == ' ').count();
        if leading_spaces >= 4 || line.starts_with('\t') {
            continue;
        }
        let trimmed = line.trim_start();

        if let Some(captures) = fence_re.captures(trimmed) {
            let fence = captures.get(1).map(|m| m.as_str()).unwrap_or_default();
            let marker = fence.chars().next().unwrap_or('`');
            let length = fence.len();

            match active_fence {
                Some((active_marker, active_len))
                    if active_marker == marker && length >= active_len =>
                {
                    active_fence = None;
                    continue;
                }
                None => {
                    active_fence = Some((marker, length));
                    continue;
                }
                _ => continue,
            }
        }

        if active_fence.is_some() {
            continue;
        }

        let Some(captures) = heading_re.captures(trimmed) else {
            continue;
        };

        let level = captures
            .get(1)
            .map(|m| m.as_str().len())
            .context("missing heading level")
            .unwrap_or(1);
        let title = captures
            .get(2)
            .map(|m| m.as_str().trim().to_string())
            .filter(|title| !title.is_empty())
            .unwrap_or_default();

        while stack
            .last()
            .is_some_and(|(existing_level, _)| *existing_level >= level)
        {
            stack.pop();
        }

        stack.push((level, title.clone()));
        headings.push(MarkdownHeading {
            title: title.clone(),
            level,
            line: index + 1,
            path: stack.iter().map(|(_, value)| value.clone()).collect(),
            slug: slugify_heading(&title),
        });
    }

    headings
}

pub fn section_end_line(
    headings: &[MarkdownHeading],
    heading_index: usize,
    total_lines: usize,
    mode: SectionSpanMode,
) -> usize {
    let current = &headings[heading_index];

    for next in headings.iter().skip(heading_index + 1) {
        let should_stop = match mode {
            SectionSpanMode::IncludeSubsections => next.level <= current.level,
            SectionSpanMode::HeadingBodyOnly => true,
        };

        if should_stop {
            return next.line.saturating_sub(1);
        }
    }

    total_lines
}

pub fn match_heading(
    headings: &[MarkdownHeading],
    heading: Option<&str>,
    heading_path: Option<&Vec<String>>,
    exact: bool,
) -> Result<usize> {
    let matches: Vec<usize> = headings
        .iter()
        .enumerate()
        .filter(|(_, candidate)| match heading_path {
            Some(path) => heading_path_matches(candidate, path, exact),
            None => heading_matches(candidate, heading.unwrap_or_default(), exact),
        })
        .map(|(index, _)| index)
        .collect();

    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(anyhow::anyhow!("Requested markdown heading was not found")),
        many => {
            let sample = many
                .iter()
                .take(5)
                .map(|index| {
                    format!(
                        "{} (line {})",
                        headings[*index].path.join(" > "),
                        headings[*index].line
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            Err(anyhow::anyhow!(
                "Requested markdown heading is ambiguous. Matching sections: {}. Use heading_path to disambiguate.",
                sample
            ))
        }
    }
}

fn heading_matches(candidate: &MarkdownHeading, requested: &str, exact: bool) -> bool {
    if exact {
        candidate.title == requested
    } else {
        candidate.title.eq_ignore_ascii_case(requested)
    }
}

fn heading_path_matches(candidate: &MarkdownHeading, requested: &[String], exact: bool) -> bool {
    if candidate.path.len() != requested.len() {
        return false;
    }

    candidate
        .path
        .iter()
        .zip(requested.iter())
        .all(|(left, right)| {
            if exact {
                left == right
            } else {
                left.eq_ignore_ascii_case(right)
            }
        })
}

fn slugify_heading(title: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;

    for ch in title.chars() {
        let lowered = ch.to_ascii_lowercase();
        if lowered.is_ascii_alphanumeric() {
            slug.push(lowered);
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}
