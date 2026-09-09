//! CATALYST: Host RPC contracts and native Turn/plugin gateways.
//! Runtime implements product methods; the native dispatcher owns initialization and execution.

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppServerNativePluginSelection {
    pub selected_plugin_ids: Vec<String>,
    pub member_plugin_ids: BTreeMap<String, Vec<String>>,
}

pub(crate) trait AppServerNativeTurnGateway: Debug + Send + Sync {
    fn start_turn<'a>(
        &'a self,
        params: TurnStartParams,
        plugin_selection: Option<AppServerNativePluginSelection>,
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

#[derive(Clone, Debug)]
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

    pub(crate) fn with_native_plugin_gateway(
        mut self,
        gateway: Arc<dyn AppServerNativePluginGateway>,
    ) -> Self {
        self.native_plugin_gateway = Some(gateway);
        self
    }

    pub(crate) fn with_native_turn_gateway(
        mut self,
        gateway: Arc<dyn AppServerNativeTurnGateway>,
    ) -> Self {
        self.native_turn_gateway = Some(gateway);
        self
    }

    pub async fn list_plugins(
        &self,
        params: PluginListParams,
    ) -> Result<PluginListResponse, JSONRPCErrorError> {
        self.native_plugin_gateway
            .as_ref()
            .ok_or_else(native_plugin_gateway_unavailable)?
            .list_plugins(params)
            .await
    }

    pub async fn loaded_plugin_roots(
        &self,
    ) -> Result<Vec<AppServerNativePluginRoot>, JSONRPCErrorError> {
        self.native_plugin_gateway
            .as_ref()
            .ok_or_else(native_plugin_gateway_unavailable)?
            .loaded_plugin_roots()
            .await
    }

    pub async fn start_turn_with_plugins(
        &self,
        params: TurnStartParams,
        selected_plugin_ids: Vec<String>,
    ) -> Result<TurnStartResponse, JSONRPCErrorError> {
        self.native_turn_gateway
            .as_ref()
            .ok_or_else(native_turn_gateway_unavailable)?
            .start_turn(
                params,
                Some(AppServerNativePluginSelection {
                    selected_plugin_ids,
                    member_plugin_ids: BTreeMap::new(),
                }),
            )
            .await
    }
}

fn native_plugin_gateway_unavailable() -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32603,
        message: "Native Plugin gateway is unavailable in this RPC context.".to_string(),
        data: None,
    }
}

fn native_turn_gateway_unavailable() -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32603,
        message: "Native turn gateway is unavailable in this RPC context.".to_string(),
        data: None,
    }
}

/// Host implementation of namespaced product methods. Register through process overrides;
/// use the supplied context to enter native Turn/plugin behavior rather than duplicating it.
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
#[path = "rpc_extension_tests.rs"]
mod tests;
