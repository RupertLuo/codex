use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::PluginListParams;
use codex_app_server_protocol::PluginListResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::client_request_methods;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;

pub type AppServerRpcFuture<'a> =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, JSONRPCErrorError>> + Send + 'a>>;
pub(crate) type AppServerNativeTurnFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TurnStartResponse, JSONRPCErrorError>> + Send + 'a>>;

pub(crate) trait AppServerNativeTurnGateway: Debug + Send + Sync {
    fn start_turn<'a>(
        &'a self,
        params: TurnStartParams,
        selected_plugin_ids: Option<Vec<String>>,
    ) -> AppServerNativeTurnFuture<'a>;
}

pub(crate) type AppServerNativePluginFuture<'a> =
    Pin<Box<dyn Future<Output = Result<PluginListResponse, JSONRPCErrorError>> + Send + 'a>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppServerNativePluginRoot {
    pub plugin_id: String,
    pub root: codex_utils_absolute_path::AbsolutePathBuf,
}

pub(crate) type AppServerNativePluginRootsFuture<'a> = Pin<
    Box<dyn Future<Output = Result<Vec<AppServerNativePluginRoot>, JSONRPCErrorError>> + Send + 'a>,
>;

pub(crate) trait AppServerNativePluginGateway: Debug + Send + Sync {
    fn list_plugins<'a>(&'a self, params: PluginListParams) -> AppServerNativePluginFuture<'a>;
    fn loaded_plugin_roots<'a>(&'a self) -> AppServerNativePluginRootsFuture<'a>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppServerRpcTransportContext {
    Stdio,
    UnixSocket,
    LoopbackWebSocket { authenticated: bool },
    AuthenticatedWebSocket,
    InProcess,
    RemoteControl,
}

#[derive(Clone)]
pub struct AppServerRpcContext {
    pub transport: AppServerRpcTransportContext,
    native_plugin_gateway: Option<Arc<dyn AppServerNativePluginGateway>>,
    native_turn_gateway: Option<Arc<dyn AppServerNativeTurnGateway>>,
}

impl AppServerRpcContext {
    pub fn new(transport: AppServerRpcTransportContext) -> Self {
        Self {
            transport,
            native_plugin_gateway: None,
            native_turn_gateway: None,
        }
    }

    pub(crate) fn with_native_turn_gateway(
        mut self,
        gateway: Arc<dyn AppServerNativeTurnGateway>,
    ) -> Self {
        self.native_turn_gateway = Some(gateway);
        self
    }

    pub(crate) fn with_native_plugin_gateway(
        mut self,
        gateway: Arc<dyn AppServerNativePluginGateway>,
    ) -> Self {
        self.native_plugin_gateway = Some(gateway);
        self
    }

    pub async fn list_plugins(
        &self,
        params: PluginListParams,
    ) -> Result<PluginListResponse, JSONRPCErrorError> {
        let gateway = self
            .native_plugin_gateway
            .as_ref()
            .ok_or_else(native_plugin_gateway_unavailable)?;
        gateway.list_plugins(params).await
    }

    pub async fn loaded_plugin_roots(
        &self,
    ) -> Result<Vec<AppServerNativePluginRoot>, JSONRPCErrorError> {
        let gateway = self
            .native_plugin_gateway
            .as_ref()
            .ok_or_else(native_plugin_gateway_unavailable)?;
        gateway.loaded_plugin_roots().await
    }

    pub async fn start_turn(
        &self,
        params: TurnStartParams,
    ) -> Result<TurnStartResponse, JSONRPCErrorError> {
        let gateway = self
            .native_turn_gateway
            .as_ref()
            .ok_or_else(native_turn_gateway_unavailable)?;
        gateway.start_turn(params, None).await
    }

    pub async fn start_turn_with_plugins(
        &self,
        params: TurnStartParams,
        selected_plugin_ids: Vec<String>,
    ) -> Result<TurnStartResponse, JSONRPCErrorError> {
        let gateway = self
            .native_turn_gateway
            .as_ref()
            .ok_or_else(native_turn_gateway_unavailable)?;
        gateway.start_turn(params, Some(selected_plugin_ids)).await
    }
}

impl Debug for AppServerRpcContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppServerRpcContext")
            .field("transport", &self.transport)
            .field(
                "native_plugin_gateway_available",
                &self.native_plugin_gateway.is_some(),
            )
            .field(
                "native_turn_gateway_available",
                &self.native_turn_gateway.is_some(),
            )
            .finish()
    }
}

fn native_turn_gateway_unavailable() -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32603,
        message: "Native turn gateway is unavailable in this RPC context.".to_string(),
        data: None,
    }
}

pub trait AppServerRpcExtension: Debug + Send + Sync {
    fn methods(&self) -> &'static [&'static str];

    fn handle<'a>(
        &'a self,
        context: AppServerRpcContext,
        method: &'a str,
        params: Option<serde_json::Value>,
    ) -> AppServerRpcFuture<'a>;
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum AppServerRpcRegistryError {
    #[error("extension RPC method must be a non-empty namespaced method: {0}")]
    InvalidMethod(String),
    #[error("extension RPC method duplicates another extension method: {0}")]
    DuplicateMethod(String),
    #[error("extension RPC method conflicts with a native method: {0}")]
    NativeMethod(String),
}

#[derive(Debug, Default)]
pub(crate) struct AppServerRpcRegistry {
    extensions: BTreeMap<&'static str, Arc<dyn AppServerRpcExtension>>,
}

impl AppServerRpcRegistry {
    pub(crate) fn new(
        extensions: Vec<Arc<dyn AppServerRpcExtension>>,
    ) -> Result<Self, AppServerRpcRegistryError> {
        let native_methods = client_request_methods()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut registered = BTreeMap::new();

        for extension in extensions {
            for &method in extension.methods() {
                if method.is_empty() || !method.contains('/') {
                    return Err(AppServerRpcRegistryError::InvalidMethod(method.to_string()));
                }
                if native_methods.contains(method) {
                    return Err(AppServerRpcRegistryError::NativeMethod(method.to_string()));
                }
                if registered.insert(method, Arc::clone(&extension)).is_some() {
                    return Err(AppServerRpcRegistryError::DuplicateMethod(
                        method.to_string(),
                    ));
                }
            }
        }

        Ok(Self {
            extensions: registered,
        })
    }

    pub(crate) fn get(&self, method: &str) -> Option<&Arc<dyn AppServerRpcExtension>> {
        self.extensions.get(method)
    }

    pub(crate) fn contains_namespace(&self, method: &str) -> bool {
        let Some((namespace, _)) = method.split_once('/') else {
            return false;
        };
        self.extensions.keys().any(|candidate| {
            candidate
                .split_once('/')
                .is_some_and(|(candidate_namespace, _)| candidate_namespace == namespace)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct StubExtension(&'static [&'static str]);

    impl AppServerRpcExtension for StubExtension {
        fn methods(&self) -> &'static [&'static str] {
            self.0
        }

        fn handle<'a>(
            &'a self,
            _context: AppServerRpcContext,
            _method: &'a str,
            _params: Option<serde_json::Value>,
        ) -> AppServerRpcFuture<'a> {
            Box::pin(async { Ok(serde_json::json!({"ok": true})) })
        }
    }

    #[test]
    fn registry_rejects_duplicate_extension_methods() {
        let result = AppServerRpcRegistry::new(vec![
            Arc::new(StubExtension(&["vendor/read"])),
            Arc::new(StubExtension(&["vendor/read"])),
        ]);
        assert!(matches!(
            result,
            Err(AppServerRpcRegistryError::DuplicateMethod(_))
        ));
    }

    #[test]
    fn registry_rejects_native_method_collisions() {
        let result = AppServerRpcRegistry::new(vec![Arc::new(StubExtension(&["thread/start"]))]);
        assert!(matches!(
            result,
            Err(AppServerRpcRegistryError::NativeMethod(_))
        ));
    }

    #[test]
    fn registry_requires_namespaced_methods() {
        let result = AppServerRpcRegistry::new(vec![Arc::new(StubExtension(&["read"]))]);
        assert!(matches!(
            result,
            Err(AppServerRpcRegistryError::InvalidMethod(_))
        ));
    }

    #[tokio::test]
    async fn rpc_context_delegates_native_turn_start_to_the_injected_gateway() {
        #[derive(Debug, Default)]
        struct StubNativeTurnGateway {
            thread_ids: Mutex<Vec<String>>,
            plugin_ids: Mutex<Vec<Option<Vec<String>>>>,
        }

        impl AppServerNativeTurnGateway for StubNativeTurnGateway {
            fn start_turn<'a>(
                &'a self,
                params: codex_app_server_protocol::TurnStartParams,
                selected_plugin_ids: Option<Vec<String>>,
            ) -> AppServerNativeTurnFuture<'a> {
                self.thread_ids.lock().unwrap().push(params.thread_id);
                self.plugin_ids.lock().unwrap().push(selected_plugin_ids);
                Box::pin(async {
                    Err(JSONRPCErrorError {
                        code: -32041,
                        message: "stub native turn".to_string(),
                        data: None,
                    })
                })
            }
        }

        let gateway = Arc::new(StubNativeTurnGateway::default());
        let context = AppServerRpcContext::new(AppServerRpcTransportContext::Stdio)
            .with_native_turn_gateway(gateway.clone());
        let error = context
            .start_turn_with_plugins(
                codex_app_server_protocol::TurnStartParams {
                    thread_id: "thread-1".to_string(),
                    ..Default::default()
                },
                vec!["drive".to_string()],
            )
            .await
            .unwrap_err();

        assert_eq!(error.code, -32041);
        assert_eq!(*gateway.thread_ids.lock().unwrap(), ["thread-1"]);
        assert_eq!(
            *gateway.plugin_ids.lock().unwrap(),
            [Some(vec!["drive".to_string()])]
        );

        let _ = context
            .start_turn(codex_app_server_protocol::TurnStartParams {
                thread_id: "thread-2".to_string(),
                ..Default::default()
            })
            .await;
        assert_eq!(gateway.plugin_ids.lock().unwrap()[1], None);
    }

    #[tokio::test]
    async fn rpc_context_delegates_native_plugin_list_to_the_injected_gateway() {
        #[derive(Debug)]
        struct StubNativePluginGateway;

        impl AppServerNativePluginGateway for StubNativePluginGateway {
            fn list_plugins<'a>(
                &'a self,
                _params: codex_app_server_protocol::PluginListParams,
            ) -> AppServerNativePluginFuture<'a> {
                Box::pin(async {
                    Ok(codex_app_server_protocol::PluginListResponse {
                        marketplaces: Vec::new(),
                        marketplace_load_errors: Vec::new(),
                        featured_plugin_ids: vec!["fixture-plugin".to_string()],
                    })
                })
            }

            fn loaded_plugin_roots<'a>(&'a self) -> AppServerNativePluginRootsFuture<'a> {
                Box::pin(async { Ok(Vec::new()) })
            }
        }

        let context = AppServerRpcContext::new(AppServerRpcTransportContext::Stdio)
            .with_native_plugin_gateway(Arc::new(StubNativePluginGateway));
        let response = context
            .list_plugins(codex_app_server_protocol::PluginListParams {
                cwds: None,
                marketplace_kinds: None,
            })
            .await
            .unwrap();

        assert_eq!(response.featured_plugin_ids, ["fixture-plugin"]);
    }

    #[tokio::test]
    async fn rpc_context_exposes_loaded_plugin_roots_without_prompt_bodies() {
        #[derive(Debug)]
        struct StubNativePluginGateway;

        impl AppServerNativePluginGateway for StubNativePluginGateway {
            fn list_plugins<'a>(
                &'a self,
                _params: codex_app_server_protocol::PluginListParams,
            ) -> AppServerNativePluginFuture<'a> {
                Box::pin(async { unreachable!() })
            }

            fn loaded_plugin_roots<'a>(&'a self) -> AppServerNativePluginRootsFuture<'a> {
                Box::pin(async {
                    Ok(vec![AppServerNativePluginRoot {
                        plugin_id: "drive".to_string(),
                        root:
                            codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path_checked(
                                std::env::temp_dir().join("drive-plugin"),
                            )
                            .unwrap(),
                    }])
                })
            }
        }

        let context = AppServerRpcContext::new(AppServerRpcTransportContext::Stdio)
            .with_native_plugin_gateway(Arc::new(StubNativePluginGateway));
        let roots = context.loaded_plugin_roots().await.unwrap();

        assert_eq!(roots[0].plugin_id, "drive");
        assert!(!format!("{roots:?}").contains("prompt"));
    }
}

fn native_plugin_gateway_unavailable() -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32603,
        message: "Native Plugin gateway is unavailable in this RPC context.".to_string(),
        data: None,
    }
}
