use codex_app_server_protocol::JSONRPCErrorError;
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
    fn start_turn<'a>(&'a self, params: TurnStartParams) -> AppServerNativeTurnFuture<'a>;
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
    native_turn_gateway: Option<Arc<dyn AppServerNativeTurnGateway>>,
}

impl AppServerRpcContext {
    pub fn new(transport: AppServerRpcTransportContext) -> Self {
        Self {
            transport,
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

    pub async fn start_turn(
        &self,
        params: TurnStartParams,
    ) -> Result<TurnStartResponse, JSONRPCErrorError> {
        let gateway = self
            .native_turn_gateway
            .as_ref()
            .ok_or_else(native_turn_gateway_unavailable)?;
        gateway.start_turn(params).await
    }
}

impl Debug for AppServerRpcContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppServerRpcContext")
            .field("transport", &self.transport)
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

/// A trusted, process-local app-server RPC extension.
pub trait AppServerRpcExtension: Debug + Send + Sync {
    /// Returns namespaced methods owned by this extension.
    fn methods(&self) -> &'static [&'static str];

    /// Handles one registered method without accessing global mutable state.
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
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut registered = BTreeMap::new();
        for extension in extensions {
            for &method in extension.methods() {
                let Some((namespace, name)) = method.split_once('/') else {
                    return Err(AppServerRpcRegistryError::InvalidMethod(method.to_string()));
                };
                if namespace.is_empty() || name.is_empty() || name.contains('/') {
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
        method.split_once('/').is_some_and(|(namespace, _)| {
            self.extensions.keys().any(|key| {
                key.split_once('/')
                    .is_some_and(|(registered, _)| registered == namespace)
            })
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
    fn registry_rejects_duplicate_and_native_methods() {
        let duplicate = AppServerRpcRegistry::new(vec![
            Arc::new(StubExtension(&["vendor/read"])),
            Arc::new(StubExtension(&["vendor/read"])),
        ]);
        assert!(matches!(
            duplicate,
            Err(AppServerRpcRegistryError::DuplicateMethod(_))
        ));

        let native = AppServerRpcRegistry::new(vec![Arc::new(StubExtension(&["thread/start"]))]);
        assert!(matches!(
            native,
            Err(AppServerRpcRegistryError::NativeMethod(_))
        ));
    }

    #[test]
    fn registry_rejects_malformed_methods() {
        for method in ["read", "/read", "vendor/", "vendor/read/extra"] {
            let result = AppServerRpcRegistry::new(vec![Arc::new(StubExtension(&[method]))]);
            assert!(matches!(
                result,
                Err(AppServerRpcRegistryError::InvalidMethod(_))
            ));
        }
    }

    #[tokio::test]
    async fn rpc_context_delegates_native_turn_start_to_the_injected_gateway() {
        #[derive(Debug, Default)]
        struct StubNativeTurnGateway {
            thread_ids: Mutex<Vec<String>>,
        }

        impl AppServerNativeTurnGateway for StubNativeTurnGateway {
            fn start_turn<'a>(
                &'a self,
                params: codex_app_server_protocol::TurnStartParams,
            ) -> AppServerNativeTurnFuture<'a> {
                self.thread_ids.lock().unwrap().push(params.thread_id);
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
            .start_turn(codex_app_server_protocol::TurnStartParams {
                thread_id: "thread-1".to_string(),
                ..Default::default()
            })
            .await
            .unwrap_err();

        assert_eq!(error.code, -32041);
        assert_eq!(*gateway.thread_ids.lock().unwrap(), ["thread-1"]);
    }
}
