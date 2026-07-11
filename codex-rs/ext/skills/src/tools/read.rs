use codex_extension_api::FunctionCallError;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolExecutorFuture;
use codex_extension_api::ToolName;
use codex_extension_api::ToolSpec;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::catalog::SkillPackageId;
use crate::catalog::SkillProviderError;
use crate::catalog::SkillResourceId;
use crate::provider::SkillReadRequest;
use crate::render::truncate_utf8_to_bytes;

use super::MAX_HANDLE_BYTES;
use super::SkillToolAuthority;
use super::SkillToolContext;
use super::external_json_output;
use super::parse_args;
use super::skill_function_tool;
use super::skill_tool_name;
use super::validate_handle;

const TOOL_NAME: &str = "read";
const MAX_PROVIDER_ERROR_CODE_BYTES: usize = 64;
const MAX_PROVIDER_ERROR_MESSAGE_BYTES: usize = 512;
const GENERIC_READ_ERROR: &str = "skill_read_failed: failed to read skill resource";

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    authority: SkillToolAuthority,
    package: String,
    resource: String,
}

#[derive(Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
struct ReadResponse {
    resource: String,
    contents: String,
}

#[derive(Clone)]
pub(super) struct ReadTool {
    pub(super) context: SkillToolContext,
}

impl ToolExecutor<ToolCall> for ReadTool {
    fn tool_name(&self) -> ToolName {
        skill_tool_name(TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        skill_function_tool::<ReadArgs, ReadResponse>(
            TOOL_NAME,
            "Read one complete resource from an enabled skill. Pass the exact authority and package returned by skills.list; resource identifiers remain opaque and are routed to that authority.",
        )
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args: ReadArgs = parse_args(&call)?;
            let authority = args.authority.to_authority()?;
            validate_handle("package", &args.package, MAX_HANDLE_BYTES)?;
            validate_handle("resource", &args.resource, MAX_HANDLE_BYTES)?;

            let requested_resource = SkillResourceId::new(args.resource);
            let result = self
                .context
                .thread_state
                .read_skill(
                    &self.context.providers,
                    SkillReadRequest {
                        authority,
                        package: SkillPackageId(args.package),
                        resource: requested_resource.clone(),
                        host_snapshot: None,
                        mcp_resources: self.context.mcp_resources.clone(),
                    },
                )
                .await
                .map_err(|err| {
                    tracing::warn!(
                        error = %err,
                        turn_id = %call.turn_id,
                        call_id = %call.call_id,
                        resource = requested_resource.as_str(),
                        "skills.read provider request failed"
                    );
                    FunctionCallError::RespondToModel(provider_error_model_message(&err))
                })?;
            if result.resource != requested_resource {
                return Err(FunctionCallError::Fatal(
                    "skill provider returned a different resource".to_string(),
                ));
            }

            external_json_output(&ReadResponse {
                resource: result.resource.as_str().to_string(),
                contents: result.contents,
            })
        })
    }
}

fn provider_error_model_message(error: &SkillProviderError) -> String {
    let Some(code) = error.code.as_deref().filter(|code| valid_error_code(code)) else {
        return GENERIC_READ_ERROR.to_string();
    };
    let Some(message) = sanitized_error_message(&error.message) else {
        return GENERIC_READ_ERROR.to_string();
    };
    format!("{code}: {message}")
}

fn valid_error_code(code: &str) -> bool {
    let mut bytes = code.bytes();
    code.len() <= MAX_PROVIDER_ERROR_CODE_BYTES
        && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn sanitized_error_message(message: &str) -> Option<String> {
    let sanitized = message
        .chars()
        .map(|character| {
            if character.is_control() || character.is_whitespace() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if sanitized.is_empty() {
        return None;
    }
    let (sanitized, _) = truncate_utf8_to_bytes(&sanitized, MAX_PROVIDER_ERROR_MESSAGE_BYTES);
    Some(sanitized)
}
