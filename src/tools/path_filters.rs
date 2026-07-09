use anyhow::{Context, Result};
use glob::Pattern;
use ignore::{WalkBuilder, overrides::OverrideBuilder};
use serde_json::Value;
use std::path::Path;

pub fn parse_pattern_strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|pattern| !pattern.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn compile_patterns(patterns: &[String]) -> Result<Vec<Pattern>> {
    patterns
        .iter()
        .map(|pattern| {
            Pattern::new(pattern).with_context(|| format!("Invalid glob pattern '{}'", pattern))
        })
        .collect()
}

pub fn apply_walk_overrides(
    walk: &mut WalkBuilder,
    root: &Path,
    includes: &[String],
    excludes: &[String],
) -> Result<()> {
    if includes.is_empty() && excludes.is_empty() {
        return Ok(());
    }

    let mut builder = OverrideBuilder::new(root);
    for pattern in includes {
        add_override(&mut builder, pattern, false)?;
    }
    for pattern in excludes {
        add_override(&mut builder, pattern, true)?;
    }

    let overrides = builder
        .build()
        .context("Invalid include/exclude walk override globs")?;
    walk.overrides(overrides);
    Ok(())
}

fn add_override(builder: &mut OverrideBuilder, pattern: &str, exclude: bool) -> Result<()> {
    let normalized = pattern.replace('\\', "/");
    let override_pattern = if exclude {
        format!("!{}", escape_leading_bang(&normalized))
    } else {
        escape_leading_bang(&normalized)
    };

    builder
        .add(&override_pattern)
        .with_context(|| format!("Invalid walk override glob '{}'", pattern))?;
    Ok(())
}

fn escape_leading_bang(pattern: &str) -> String {
    if pattern.starts_with('!') {
        format!("\\{}", pattern)
    } else {
        pattern.to_string()
    }
}
