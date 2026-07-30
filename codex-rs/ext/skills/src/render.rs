use std::borrow::Cow;

use codex_core_skills::BudgetedSkillLine;
use codex_core_skills::SkillMetadataBudget;
use codex_core_skills::render_budgeted_skill_lines;
use codex_utils_string::take_bytes_at_char_boundary;

use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillSourceKind;
use crate::fragments::AvailableSkillsInstructions;

const MAX_MAIN_PROMPT_BYTES: usize = 8_000;
const MAX_CATALOG_SKILL_DESCRIPTION_CHARS: usize = 1_024;
const TRUNCATED_SKILL_DESCRIPTION_SUFFIX: &str = "...";
pub(crate) const MAX_SKILL_NAME_BYTES: usize = 256;
pub(crate) const MAX_SKILL_PATH_BYTES: usize = 1_024;

#[tracing::instrument(
    level = "trace",
    skip_all,
    fields(catalog_entry_count = catalog.entries.len())
)]
pub(crate) fn available_skills_fragment(
    catalog: &SkillCatalog,
    budget: SkillMetadataBudget,
    include_skills_usage_instructions: bool,
) -> Option<AvailableSkillsInstructions> {
    // Shortening descriptions before dropping entries is the whole point of going through the
    // shared renderer: a skill the model never sees named cannot be chosen at all, whereas one
    // with a shortened description still can. This list used to be capped at a flat byte count,
    // which measured CJK descriptions at three bytes per character and dropped whole skills.
    let lines = catalog
        .entries
        .iter()
        .filter(|entry| entry.enabled && entry.prompt_visible)
        .map(|entry| BudgetedSkillLine {
            name: entry.name.as_str(),
            description: entry
                .short_description
                .as_deref()
                .unwrap_or(entry.description.as_str()),
            locator_kind: locator_kind(entry),
            path: entry.rendered_path().to_string(),
        })
        .collect::<Vec<_>>();

    let (mut skill_lines, report) = render_budgeted_skill_lines(lines, budget);
    if skill_lines.is_empty() {
        return None;
    }
    if report.omitted_count > 0 {
        let omitted = report.omitted_count;
        let skill_word = if omitted == 1 { "skill" } else { "skills" };
        skill_lines.push(format!(
            "- {omitted} additional {skill_word} omitted from this bounded skills list."
        ));
    }

    Some(AvailableSkillsInstructions::from_skill_lines(
        skill_lines,
        include_skills_usage_instructions,
    ))
}

pub(crate) fn truncate_catalog_skill_description(description: &str) -> Cow<'_, str> {
    if description
        .char_indices()
        .nth(MAX_CATALOG_SKILL_DESCRIPTION_CHARS)
        .is_none()
    {
        return Cow::Borrowed(description);
    }

    let prefix_chars = MAX_CATALOG_SKILL_DESCRIPTION_CHARS
        .saturating_sub(TRUNCATED_SKILL_DESCRIPTION_SUFFIX.chars().count());
    let prefix_end = description
        .char_indices()
        .nth(prefix_chars)
        .map_or(description.len(), |(index, _)| index);
    let mut truncated = description[..prefix_end].to_string();
    truncated.push_str(TRUNCATED_SKILL_DESCRIPTION_SUFFIX);
    Cow::Owned(truncated)
}

/// The word the prompt uses before a skill's locator, so the model knows how to open it.
fn locator_kind(entry: &SkillCatalogEntry) -> &'static str {
    match &entry.authority.kind {
        SkillSourceKind::Host => "file",
        SkillSourceKind::Executor => "environment resource",
        SkillSourceKind::Orchestrator => "orchestrator resource",
        SkillSourceKind::Custom(_) => "custom resource",
    }
}

pub(crate) fn truncate_main_prompt_contents(contents: &str) -> (String, bool) {
    truncate_utf8_to_bytes(contents, MAX_MAIN_PROMPT_BYTES)
}

pub(crate) fn truncate_utf8_to_bytes(contents: &str, max_bytes: usize) -> (String, bool) {
    let truncated = take_bytes_at_char_boundary(contents, max_bytes);
    (truncated.to_string(), truncated.len() < contents.len())
}
