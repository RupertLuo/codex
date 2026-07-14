use codex_core_skills::SKILLS_HOW_TO_USE_WITH_ABSOLUTE_PATHS;
use codex_core_skills::render_available_skills_body;
use codex_extension_api::ContextualUserFragment;
use codex_protocol::protocol::SKILLS_INSTRUCTIONS_CLOSE_TAG;
use codex_protocol::protocol::SKILLS_INSTRUCTIONS_OPEN_TAG;

use crate::tools::SkillToolAddress;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AvailableSkillsInstructions {
    skill_lines: Vec<String>,
}

impl AvailableSkillsInstructions {
    pub(crate) fn from_skill_lines(
        mut skill_lines: Vec<String>,
        include_skills_usage_instructions: bool,
    ) -> Self {
        if include_skills_usage_instructions {
            skill_lines.push("### How to use skills".to_string());
            skill_lines.push(SKILLS_HOW_TO_USE_WITH_ABSOLUTE_PATHS.to_string());
        }
        Self { skill_lines }
    }
}

impl ContextualUserFragment for AvailableSkillsInstructions {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (SKILLS_INSTRUCTIONS_OPEN_TAG, SKILLS_INSTRUCTIONS_CLOSE_TAG)
    }

    fn body(&self) -> String {
        render_available_skills_body(&[], &self.skill_lines)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillInstructions {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) access: Option<SkillAccess>,
    pub(crate) contents: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillAccess {
    authority_kind: String,
    package: String,
    main_resource: String,
}

impl From<SkillToolAddress> for SkillAccess {
    fn from(address: SkillToolAddress) -> Self {
        Self {
            authority_kind: address.authority.kind().to_string(),
            package: address.package,
            main_resource: address.main_resource,
        }
    }
}

impl ContextualUserFragment for SkillInstructions {
    fn role(&self) -> &'static str {
        "user"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<skill>", "</skill>")
    }

    fn body(&self) -> String {
        let name = &self.name;
        let path = &self.path;
        let contents = &self.contents;
        let access = self.access.as_ref().map_or_else(String::new, |access| {
            let authority_kind = escape_xml_text(&access.authority_kind);
            let package = escape_xml_text(&access.package);
            let main_resource = escape_xml_text(&access.main_resource);
            format!(
                "<access>\n<authority-kind>{authority_kind}</authority-kind>\n<package>{package}</package>\n<main-resource>{main_resource}</main-resource>\n</access>\n"
            )
        });
        format!("\n<name>{name}</name>\n<path>{path}</path>\n{access}{contents}\n")
    }
}

fn escape_xml_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}
