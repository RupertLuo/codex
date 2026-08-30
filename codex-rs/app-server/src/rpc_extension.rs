use codex_app_server_protocol::JSONRPCErrorError;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppServerRpcTransportContext {
    Stdio,
    UnixSocket,
    LoopbackWebSocket { authenticated: bool },
    AuthenticatedWebSocket,
    InProcess,
    RemoteControl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppServerRpcContext {
    pub transport: AppServerRpcTransportContext,
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
}
