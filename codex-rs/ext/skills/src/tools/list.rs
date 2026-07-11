use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolExecutorFuture;
use codex_extension_api::ToolName;
use codex_extension_api::ToolSpec;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::catalog::SkillCatalogEntry;
use crate::render::truncate_catalog_skill_description;
use crate::render::truncate_utf8_to_bytes;

use super::MAX_HANDLE_BYTES;
use super::SkillToolAuthority;
use super::SkillToolContext;
use super::external_json_output;
use super::is_bounded_handle;
use super::parse_args;
use super::skill_function_tool;
use super::skill_tool_name;

const TOOL_NAME: &str = "list";
const MAX_WARNINGS: usize = 4;
const MAX_WARNING_BYTES: usize = 256;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    authority: SkillToolAuthority,
}

#[derive(Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
struct ListedSkill {
    authority: SkillToolAuthority,
    package: String,
    name: String,
    description: String,
    main_resource: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<ListedSkillDependency>,
}

#[derive(Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
struct ListedSkillDependency {
    authority: SkillToolAuthority,
    package: String,
}

#[derive(Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
struct ListResponse {
    skills: Vec<ListedSkill>,
    warnings: Vec<String>,
}

#[derive(Clone)]
pub(super) struct ListTool {
    pub(super) context: SkillToolContext,
}

impl ToolExecutor<ToolCall> for ListTool {
    fn tool_name(&self) -> ToolName {
        skill_tool_name(TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        skill_function_tool::<ListArgs, ListResponse>(
            TOOL_NAME,
            "List enabled skills owned by an orchestrator or custom authority. Use authority kind `custom` to discover every custom authority. Reuse each returned authority, opaque package, and main-resource handle exactly when calling skills.read.",
        )
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args: ListArgs = parse_args(&call)?;
            args.authority.to_authority()?;
            let requested_authority = args.authority.clone();
            let catalog = self.context.catalog(&call.turn_id, args.authority).await;
            let mut warnings = catalog.warnings;
            let response = ListResponse {
                skills: catalog
                    .entries
                    .into_iter()
                    .filter(|entry| {
                        entry.enabled && requested_authority.matches_authority(&entry.authority)
                    })
                    .filter_map(|entry| listed_skill(entry, &mut warnings))
                    .collect(),
                warnings: bounded_warnings(warnings),
            };

            external_json_output(&response)
        })
    }
}

fn listed_skill(entry: SkillCatalogEntry, warnings: &mut Vec<String>) -> Option<ListedSkill> {
    let authority = SkillToolAuthority::from_authority(&entry.authority)?;
    if !is_bounded_handle(&entry.id.0, MAX_HANDLE_BYTES)
        || !is_bounded_handle(entry.main_prompt.as_str(), MAX_HANDLE_BYTES)
    {
        return None;
    }
    let mut omitted_dependency = false;
    let dependencies = entry
        .package_dependencies
        .into_iter()
        .filter_map(|dependency| {
            let Some(authority) = SkillToolAuthority::from_authority(&dependency.authority) else {
                omitted_dependency = true;
                return None;
            };
            if !is_bounded_handle(&dependency.package.0, MAX_HANDLE_BYTES) {
                omitted_dependency = true;
                return None;
            }
            Some(ListedSkillDependency {
                authority,
                package: dependency.package.0,
            })
        })
        .collect();
    if omitted_dependency {
        warnings.push(invalid_dependency_warning());
    }

    Some(ListedSkill {
        authority,
        package: entry.id.0,
        name: entry.name,
        description: truncate_catalog_skill_description(&entry.description).into_owned(),
        main_resource: entry.main_prompt.as_str().to_string(),
        dependencies,
    })
}

fn invalid_dependency_warning() -> String {
    "skill dependency omitted because its authority or package handle is not tool-addressable"
        .to_string()
}

fn bounded_warnings(warnings: Vec<String>) -> Vec<String> {
    warnings
        .into_iter()
        .take(MAX_WARNINGS)
        .map(|warning| {
            let (warning, _) = truncate_utf8_to_bytes(&warning, MAX_WARNING_BYTES);
            warning
        })
        .collect()
}
