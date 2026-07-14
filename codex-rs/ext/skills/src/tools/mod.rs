use std::sync::Arc;

use codex_extension_api::FunctionCallError;
use codex_extension_api::JsonToolOutput;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_extension_api::parse_tool_input_schema;
use codex_mcp::CODEX_APPS_MCP_SERVER_NAME;
use codex_mcp::McpResourceClient;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::default_namespace_description;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillSourceKind;
use crate::provider::SkillListQuery;
use crate::sources::SkillProviders;
use crate::state::SkillsThreadState;

mod list;
mod read;
mod schema;

const SKILLS_NAMESPACE: &str = "skills";
const MAX_HANDLE_BYTES: usize = 2_048;

pub(crate) fn skill_tools(
    providers: SkillProviders,
    mcp_resources: Option<Arc<McpResourceClient>>,
    thread_state: Arc<SkillsThreadState>,
) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
    let context = SkillToolContext {
        providers,
        mcp_resources,
        thread_state,
    };
    vec![
        Arc::new(list::ListTool {
            context: context.clone(),
        }),
        Arc::new(read::ReadTool { context }),
    ]
}

#[derive(Clone)]
struct SkillToolContext {
    providers: SkillProviders,
    mcp_resources: Option<Arc<McpResourceClient>>,
    thread_state: Arc<SkillsThreadState>,
}

impl SkillToolContext {
    async fn catalog(&self, turn_id: &str, authority: SkillToolAuthority) -> SkillCatalog {
        match authority.kind.as_str() {
            "orchestrator" => {
                let generation = self
                    .mcp_resources
                    .as_ref()
                    .map(|client| client.capture_generation());
                self.thread_state
                    .orchestrator_catalog_snapshot(
                        generation.as_ref(),
                        self.providers.list_orchestrator_for_turn(SkillListQuery {
                            turn_id: turn_id.to_string(),
                            executor_roots: Vec::new(),
                            host_snapshot: None,
                            include_host_skills: false,
                            include_bundled_skills: false,
                            include_orchestrator_skills: true,
                            mcp_resources: self.mcp_resources.clone(),
                            mcp_resource_generation: generation.clone(),
                        }),
                    )
                    .await
            }
            "custom" => {
                self.providers
                    .list_all_custom_for_turn(SkillListQuery {
                        turn_id: turn_id.to_string(),
                        executor_roots: Vec::new(),
                        host_snapshot: None,
                        include_host_skills: false,
                        include_bundled_skills: false,
                        include_orchestrator_skills: false,
                        mcp_resources: self.mcp_resources.clone(),
                        mcp_resource_generation: None,
                    })
                    .await
            }
            custom_kind => {
                let kind = SkillSourceKind::Custom(custom_kind.to_string());
                self.providers
                    .list_custom_for_turn(
                        SkillListQuery {
                            turn_id: turn_id.to_string(),
                            executor_roots: Vec::new(),
                            host_snapshot: None,
                            include_host_skills: false,
                            include_bundled_skills: false,
                            include_orchestrator_skills: false,
                            mcp_resources: self.mcp_resources.clone(),
                            mcp_resource_generation: None,
                        },
                        &kind,
                    )
                    .await
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub(crate) struct SkillToolAuthority {
    kind: String,
}

impl SkillToolAuthority {
    pub(crate) fn from_authority(authority: &SkillAuthority) -> Option<Self> {
        match &authority.kind {
            SkillSourceKind::Orchestrator if authority.id == CODEX_APPS_MCP_SERVER_NAME => {
                Some(Self {
                    kind: "orchestrator".to_string(),
                })
            }
            SkillSourceKind::Custom(kind)
                if authority.id == *kind && is_bounded_handle(kind, MAX_HANDLE_BYTES) =>
            {
                Some(Self { kind: kind.clone() })
            }
            SkillSourceKind::Host
            | SkillSourceKind::Executor
            | SkillSourceKind::Orchestrator
            | SkillSourceKind::Custom(_) => None,
        }
    }

    fn to_authority(&self) -> Result<SkillAuthority, FunctionCallError> {
        validate_handle("authority.kind", &self.kind, MAX_HANDLE_BYTES)?;
        match self.kind.as_str() {
            "orchestrator" => Ok(SkillAuthority::new(
                SkillSourceKind::Orchestrator,
                CODEX_APPS_MCP_SERVER_NAME,
            )),
            "host" | "executor" => Err(FunctionCallError::RespondToModel(
                "skills tools do not support host or executor authorities".to_string(),
            )),
            custom_kind => Ok(SkillAuthority::new(
                SkillSourceKind::Custom(custom_kind.to_string()),
                custom_kind,
            )),
        }
    }

    fn matches_authority(&self, authority: &SkillAuthority) -> bool {
        match self.kind.as_str() {
            "custom" => matches!(
                &authority.kind,
                SkillSourceKind::Custom(kind) if authority.id == *kind
            ),
            _ => Self::from_authority(authority).as_ref() == Some(self),
        }
    }

    pub(crate) fn kind(&self) -> &str {
        &self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillToolAddress {
    pub(crate) authority: SkillToolAuthority,
    pub(crate) package: String,
    pub(crate) main_resource: String,
}

impl SkillToolAddress {
    pub(crate) fn from_entry(entry: &SkillCatalogEntry) -> Option<Self> {
        let authority = SkillToolAuthority::from_authority(&entry.authority)?;
        if !is_bounded_handle(&entry.id.0, MAX_HANDLE_BYTES)
            || !is_bounded_handle(entry.main_prompt.as_str(), MAX_HANDLE_BYTES)
        {
            return None;
        }
        Some(Self {
            authority,
            package: entry.id.0.clone(),
            main_resource: entry.main_prompt.as_str().to_string(),
        })
    }
}

fn skill_tool_name(name: &str) -> ToolName {
    ToolName::namespaced(SKILLS_NAMESPACE, name)
}

fn skill_function_tool<I: JsonSchema, O: JsonSchema>(name: &str, description: &str) -> ToolSpec {
    let tool = ResponsesApiTool {
        name: name.to_string(),
        description: description.to_string(),
        strict: false,
        defer_loading: None,
        parameters: parse_tool_input_schema(&schema::input_schema_for::<I>())
            .unwrap_or_else(|err| panic!("generated input schema for {name} should parse: {err}")),
        output_schema: Some(schema::output_schema_for::<O>()),
    };

    ToolSpec::Namespace(ResponsesApiNamespace {
        name: SKILLS_NAMESPACE.to_string(),
        description: default_namespace_description(SKILLS_NAMESPACE),
        tools: vec![ResponsesApiNamespaceTool::Function(tool)],
    })
}

fn parse_args<T: for<'de> Deserialize<'de>>(call: &ToolCall) -> Result<T, FunctionCallError> {
    let arguments = call.function_arguments()?;
    let value = if arguments.trim().is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(arguments)
            .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?
    };
    serde_json::from_value(value).map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

fn validate_handle(name: &str, value: &str, max_bytes: usize) -> Result<(), FunctionCallError> {
    if is_bounded_handle(value, max_bytes) {
        return Ok(());
    }

    Err(FunctionCallError::RespondToModel(format!(
        "{name} must be non-empty, contain no control characters, and be at most {max_bytes} bytes"
    )))
}

fn is_bounded_handle(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn external_json_output<T: Serialize>(value: &T) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    let value = serde_json::to_value(value).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize tool output: {err}"))
    })?;
    Ok(Box::new(JsonToolOutput::new(value).with_external_context()))
}
