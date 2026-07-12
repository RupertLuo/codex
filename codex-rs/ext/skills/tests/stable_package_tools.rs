use std::sync::Arc;

use codex_extension_api::ConversationHistory;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::FunctionCallError;
use codex_extension_api::NoopTurnItemEmitter;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolPayload;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TruncationPolicy;
use codex_skills_extension::OrchestratorSkillProvider;
use codex_skills_extension::SkillProviderSource;
use codex_skills_extension::SkillProviders;
use codex_skills_extension::SkillsExtensionConfig;
use codex_skills_extension::catalog::SkillAuthority;
use codex_skills_extension::catalog::SkillCatalog;
use codex_skills_extension::catalog::SkillCatalogEntry;
use codex_skills_extension::catalog::SkillPackageDependency;
use codex_skills_extension::catalog::SkillPackageId;
use codex_skills_extension::catalog::SkillProviderError;
use codex_skills_extension::catalog::SkillReadResult;
use codex_skills_extension::catalog::SkillResourceId;
use codex_skills_extension::catalog::SkillSearchResult;
use codex_skills_extension::catalog::SkillSourceKind;
use codex_skills_extension::install_with_providers;
use codex_skills_extension::provider::SkillListQuery;
use codex_skills_extension::provider::SkillProvider;
use codex_skills_extension::provider::SkillProviderFuture;
use codex_skills_extension::provider::SkillReadRequest;
use codex_skills_extension::provider::SkillSearchRequest;
use pretty_assertions::assert_eq;
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const PRIVATE_KIND: &str = "private-provider";
const PARENT_PACKAGE: &str = "private/parent";
const PARENT_RESOURCE: &str = "skill://private/parent/SKILL.md";

#[tokio::test]
async fn package_listed_in_one_turn_can_be_read_in_a_later_turn() -> TestResult {
    let tools = tools(Arc::new(FakePrivateProvider::default())).await;
    let list_tool = tool(&tools, "list")?;
    let listed = call_json(
        list_tool,
        "turn-a",
        "list-a",
        serde_json::json!({"authority": {"kind": PRIVATE_KIND}}),
    )
    .await?;

    assert_eq!(
        listed["skills"],
        serde_json::json!([{
            "authority": {"kind": PRIVATE_KIND},
            "package": PARENT_PACKAGE,
            "name": "parent",
            "description": "Parent Skill",
            "main_resource": PARENT_RESOURCE,
            "dependencies": [{
                "authority": {"kind": PRIVATE_KIND},
                "package": "private/child"
            }]
        }])
    );

    let read = call_json(
        tool(&tools, "read")?,
        "turn-b",
        "read-b",
        serde_json::json!({
            "authority": {"kind": PRIVATE_KIND},
            "package": PARENT_PACKAGE,
            "resource": PARENT_RESOURCE
        }),
    )
    .await?;
    assert_eq!(
        read,
        serde_json::json!({"resource": PARENT_RESOURCE, "contents": "parent body"})
    );

    Ok(())
}

#[tokio::test]
async fn coded_provider_error_is_model_visible_without_provider_paths() -> TestResult {
    let tools = tools(Arc::new(FakePrivateProvider::default())).await;
    let read_tool = tool(&tools, "read")?;
    let error = match call(
        read_tool,
        "turn-b",
        "read-legacy",
        serde_json::json!({
            "authority": {"kind": PRIVATE_KIND},
            "package": "private/legacy",
            "resource": "skill://private/legacy/SKILL.md"
        }),
    )
    .await
    {
        Ok(_) => panic!("legacy package should fail"),
        Err(error) => error,
    };
    let FunctionCallError::RespondToModel(message) = error else {
        panic!("provider failure should be returned to the model");
    };

    assert!(message.starts_with("private_skill_handle_legacy: "));
    assert!(!message.contains("/Users/provider/private-skills"));
    Ok(())
}

#[tokio::test]
async fn uncoded_provider_error_uses_generic_model_message() -> TestResult {
    let tools = tools(Arc::new(FakePrivateProvider::default())).await;
    let error = match call(
        tool(&tools, "read")?,
        "turn-b",
        "read-failed",
        serde_json::json!({
            "authority": {"kind": PRIVATE_KIND},
            "package": "private/failed",
            "resource": "skill://private/failed/SKILL.md"
        }),
    )
    .await
    {
        Ok(_) => panic!("provider read should fail"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        FunctionCallError::RespondToModel(
            "skill_read_failed: failed to read skill resource".to_string()
        )
    );
    Ok(())
}

#[tokio::test]
async fn invalid_dependency_omits_entire_parent_and_bounds_warnings() -> TestResult {
    let tools = tools(Arc::new(FakePrivateProvider::with_invalid_dependency())).await;
    let listed = call_json(
        tool(&tools, "list")?,
        "turn-a",
        "list-invalid-dependency",
        serde_json::json!({"authority": {"kind": PRIVATE_KIND}}),
    )
    .await?;

    assert_eq!(
        listed["skills"],
        serde_json::json!([{
            "authority": {"kind": PRIVATE_KIND},
            "package": "private/safe",
            "name": "safe",
            "description": "Safe Skill",
            "main_resource": "skill://private/safe/SKILL.md"
        }])
    );
    let warnings = listed["warnings"]
        .as_array()
        .ok_or("warnings should be an array")?;
    assert_eq!(warnings.len(), 4);
    assert!(
        warnings[0]
            .as_str()
            .is_some_and(|warning| warning.contains("Skill was omitted") && warning.len() <= 256)
    );
    assert!(
        warnings
            .iter()
            .all(|warning| warning.as_str().is_some_and(|warning| warning.len() <= 256))
    );
    Ok(())
}

#[tokio::test]
async fn dependency_output_is_capped_per_entry_and_across_the_list() -> TestResult {
    let tools = tools(Arc::new(FakePrivateProvider::with_many_dependencies())).await;
    let listed = call_json(
        tool(&tools, "list")?,
        "turn-a",
        "list-many-dependencies",
        serde_json::json!({"authority": {"kind": PRIVATE_KIND}}),
    )
    .await?;

    let skills = listed["skills"]
        .as_array()
        .ok_or("skills should be an array")?;
    assert_eq!(skills.len(), 5);
    assert_eq!(skills[0]["dependencies"].as_array().map(Vec::len), Some(32));
    assert_eq!(
        skills
            .iter()
            .filter_map(|skill| skill["dependencies"].as_array())
            .map(Vec::len)
            .sum::<usize>(),
        100
    );
    let truncation_warnings = listed["warnings"]
        .as_array()
        .ok_or("warnings should be an array")?
        .iter()
        .filter_map(Value::as_str)
        .filter(|warning| warning.contains("truncated"))
        .collect::<Vec<_>>();
    assert_eq!(truncation_warnings.len(), 1);
    assert!(truncation_warnings[0].len() <= 256);
    Ok(())
}

#[tokio::test]
async fn orchestrator_provider_rejects_unlisted_package_before_transport_access() -> TestResult {
    let provider = OrchestratorSkillProvider::new();
    let error = provider
        .read(SkillReadRequest {
            authority: SkillAuthority::new(SkillSourceKind::Orchestrator, "codex_apps"),
            package: SkillPackageId("skill://apps/private".to_string()),
            resource: SkillResourceId::new("skill://apps/private/SKILL.md"),
            host_snapshot: None,
            mcp_resources: None,
            mcp_resource_generation: None,
        })
        .await
        .expect_err("unlisted orchestrator package should be rejected");

    assert_eq!(
        error.message,
        "orchestrator skill package is not authorized by discovery"
    );
    Ok(())
}

#[tokio::test]
async fn coded_provider_error_is_sanitized_and_bounded_for_the_model() -> TestResult {
    let tools = tools(Arc::new(FakePrivateProvider::default())).await;
    let error = match call(
        tool(&tools, "read")?,
        "turn-b",
        "read-unsafe-error",
        serde_json::json!({
            "authority": {"kind": PRIVATE_KIND},
            "package": "private/unsafe-error",
            "resource": "skill://private/unsafe-error/SKILL.md"
        }),
    )
    .await
    {
        Ok(_) => panic!("unsafe provider error should fail"),
        Err(error) => error,
    };
    let FunctionCallError::RespondToModel(message) = error else {
        panic!("provider failure should be returned to the model");
    };

    assert!(message.starts_with("private_skill_handle_legacy: "));
    assert!(message.len() <= 64 + 2 + 512);
    assert!(!message.chars().any(char::is_control));
    assert!(!message.contains("/Users/provider/private-skills"));
    Ok(())
}

#[tokio::test]
async fn invalid_provider_error_code_uses_generic_model_message() -> TestResult {
    let tools = tools(Arc::new(FakePrivateProvider::default())).await;
    for package in ["private/invalid-code", "private/oversized-code"] {
        let error = match call(
            tool(&tools, "read")?,
            "turn-b",
            "read-invalid-code",
            serde_json::json!({
                "authority": {"kind": PRIVATE_KIND},
                "package": package,
                "resource": format!("skill://{package}/SKILL.md")
            }),
        )
        .await
        {
            Ok(_) => panic!("invalid coded provider error should fail"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            FunctionCallError::RespondToModel(
                "skill_read_failed: failed to read skill resource".to_string()
            )
        );
    }
    Ok(())
}

#[derive(Default)]
struct FakePrivateProvider {
    invalid_dependency: bool,
    many_dependencies: bool,
}

impl FakePrivateProvider {
    fn with_invalid_dependency() -> Self {
        Self {
            invalid_dependency: true,
            many_dependencies: false,
        }
    }

    fn with_many_dependencies() -> Self {
        Self {
            invalid_dependency: false,
            many_dependencies: true,
        }
    }
}

impl SkillProvider for FakePrivateProvider {
    fn list(&self, query: SkillListQuery) -> SkillProviderFuture<'_, SkillCatalog> {
        let invalid_dependency = self.invalid_dependency;
        let many_dependencies = self.many_dependencies;
        Box::pin(async move {
            if many_dependencies {
                return Ok(SkillCatalog {
                    entries: (0..5)
                        .map(|parent| {
                            SkillCatalogEntry::new(
                                SkillPackageId(format!("private/parent-{parent}")),
                                private_authority(),
                                format!("parent-{parent}"),
                                "Parent Skill",
                                SkillResourceId::new(format!(
                                    "skill://private/parent-{parent}/SKILL.md"
                                )),
                            )
                            .with_package_dependencies(
                                (0..40)
                                    .map(|child| SkillPackageDependency {
                                        authority: private_authority(),
                                        package: SkillPackageId(format!(
                                            "private/parent-{parent}/child-{child}"
                                        )),
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                    warnings: Vec::new(),
                });
            }
            let entry = if query.turn_id == "turn-a" {
                let dependencies = if invalid_dependency {
                    vec![
                        SkillPackageDependency {
                            authority: private_authority(),
                            package: SkillPackageId("private/valid-before".to_string()),
                        },
                        SkillPackageDependency {
                            authority: SkillAuthority::new(SkillSourceKind::Host, "host"),
                            package: SkillPackageId("private/child".to_string()),
                        },
                        SkillPackageDependency {
                            authority: private_authority(),
                            package: SkillPackageId("private/valid-after".to_string()),
                        },
                    ]
                } else {
                    vec![SkillPackageDependency {
                        authority: private_authority(),
                        package: SkillPackageId("private/child".to_string()),
                    }]
                };
                SkillCatalogEntry::new(
                    SkillPackageId(PARENT_PACKAGE.to_string()),
                    private_authority(),
                    "parent",
                    "Parent Skill",
                    SkillResourceId::new(PARENT_RESOURCE),
                )
                .with_package_dependencies(dependencies)
            } else {
                SkillCatalogEntry::new(
                    SkillPackageId("private/current-turn".to_string()),
                    private_authority(),
                    "current-turn",
                    "Current Turn Skill",
                    SkillResourceId::new("skill://private/current-turn/SKILL.md"),
                )
            };
            let entries = if invalid_dependency {
                vec![
                    entry,
                    SkillCatalogEntry::new(
                        SkillPackageId("private/safe".to_string()),
                        private_authority(),
                        "safe",
                        "Safe Skill",
                        SkillResourceId::new("skill://private/safe/SKILL.md"),
                    ),
                ]
            } else {
                vec![entry]
            };
            let warnings = if invalid_dependency {
                (0..8)
                    .map(|index| format!("catalog warning {index}: {}", "x".repeat(512)))
                    .collect()
            } else {
                Vec::new()
            };
            Ok(SkillCatalog { entries, warnings })
        })
    }

    fn read(&self, request: SkillReadRequest) -> SkillProviderFuture<'_, SkillReadResult> {
        Box::pin(async move {
            match request.package.0.as_str() {
                "private/legacy" => Err(SkillProviderError::coded(
                    "private_skill_handle_legacy",
                    "The private Skill handle is from the retired turn-scoped format; call skills.list again.",
                )),
                "private/failed" => Err(SkillProviderError::new(
                    "failed below /Users/provider/private-skills/parent/SKILL.md",
                )),
                "private/unsafe-error" => Err(SkillProviderError::coded_with_internal(
                    "private_skill_handle_legacy",
                    format!("line one\nline two\u{1b} {}", "x".repeat(1_024)),
                    "failed below /Users/provider/private-skills/parent/SKILL.md",
                )),
                "private/invalid-code" => Err(SkillProviderError::coded_with_internal(
                    "INVALID\nCODE",
                    "public text",
                    "failed below /Users/provider/private-skills/parent/SKILL.md",
                )),
                "private/oversized-code" => Err(SkillProviderError::coded_with_internal(
                    "a".repeat(65),
                    "public text",
                    "failed below /Users/provider/private-skills/parent/SKILL.md",
                )),
                _ => Ok(SkillReadResult {
                    resource: request.resource,
                    contents: "parent body".to_string(),
                }),
            }
        })
    }

    fn search(&self, _request: SkillSearchRequest) -> SkillProviderFuture<'_, SkillSearchResult> {
        Box::pin(async { Ok(SkillSearchResult::default()) })
    }
}

fn private_authority() -> SkillAuthority {
    SkillAuthority::new(SkillSourceKind::custom(PRIVATE_KIND), PRIVATE_KIND)
}

async fn tools(provider: Arc<dyn SkillProvider>) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
    let providers = SkillProviders::new().with_provider(SkillProviderSource::new(
        SkillSourceKind::custom(PRIVATE_KIND),
        PRIVATE_KIND,
        provider,
    ));
    let mut builder = ExtensionRegistryBuilder::new();
    install_with_providers(&mut builder, providers, |_| SkillsExtensionConfig {
        include_instructions: true,
        bundled_skills_enabled: true,
        orchestrator_skills_enabled: true,
    });
    let registry = builder.build();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &(),
            session_source: &SessionSource::Cli,
            persistent_thread_state_available: true,
            environments: &[],
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;
    registry.tool_contributors()[0].tools(&session_store, &thread_store)
}

fn tool<'a>(
    tools: &'a [Arc<dyn ToolExecutor<ToolCall>>],
    name: &str,
) -> TestResult<&'a Arc<dyn ToolExecutor<ToolCall>>> {
    tools
        .iter()
        .find(|tool| tool.tool_name().name == name)
        .ok_or_else(|| format!("skills.{name} tool should be registered").into())
}

async fn call_json(
    tool: &Arc<dyn ToolExecutor<ToolCall>>,
    turn_id: &str,
    call_id: &str,
    arguments: Value,
) -> TestResult<Value> {
    let payload = ToolPayload::Function {
        arguments: arguments.to_string(),
    };
    let output = call(tool, turn_id, call_id, arguments).await?;
    output
        .post_tool_use_response(call_id, &payload)
        .ok_or_else(|| "tool should expose structured output".into())
}

async fn call(
    tool: &Arc<dyn ToolExecutor<ToolCall>>,
    turn_id: &str,
    call_id: &str,
    arguments: Value,
) -> Result<Box<dyn codex_extension_api::ToolOutput>, FunctionCallError> {
    tool.handle(ToolCall {
        turn_id: turn_id.to_string(),
        call_id: call_id.to_string(),
        tool_name: tool.tool_name(),
        model: "gpt-test".to_string(),
        truncation_policy: TruncationPolicy::Bytes(32_768),
        conversation_history: ConversationHistory::default(),
        turn_item_emitter: Arc::new(NoopTurnItemEmitter),
        environments: Vec::new(),
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    })
    .await
}
