use std::sync::Arc;
use std::sync::Weak;

use anyhow::Context;
use anyhow::Result;
use arc_swap::ArcSwap;
use codex_protocol::mcp::Resource;
use codex_protocol::mcp::ResourceContent;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ReadResourceRequestParams;

use crate::McpConnectionManager;

/// One page of resources returned by an MCP server.
#[derive(Clone, Debug, PartialEq)]
pub struct McpResourcePage {
    /// Resources advertised on this page.
    pub resources: Vec<Resource>,
    /// Opaque cursor to supply when requesting the next page.
    pub next_cursor: Option<String>,
}

/// Contents returned after reading one MCP resource.
#[derive(Clone, Debug, PartialEq)]
pub struct McpResourceReadResult {
    /// Text or blob content returned for the requested resource.
    pub contents: Vec<ResourceContent>,
}

/// Session-scoped access to MCP resources through the currently installed manager.
///
/// The client retains the manager's shared publication handle rather than a manager
/// snapshot, so calls automatically use replacements installed during startup and refresh.
#[derive(Clone)]
pub struct McpResourceClient {
    manager: Arc<ArcSwap<McpConnectionManager>>,
}

/// One stable MCP connection-manager generation used for a multi-call operation.
#[derive(Clone)]
pub struct McpResourceClientGeneration {
    manager: Arc<McpConnectionManager>,
}

/// Opaque identity for the manager currently used by an MCP resource client.
#[derive(Clone)]
pub struct McpResourceClientCacheKey(Weak<McpConnectionManager>);

impl PartialEq for McpResourceClientCacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.ptr_eq(&other.0)
    }
}

impl Eq for McpResourceClientCacheKey {}

impl McpResourceClientCacheKey {
    /// Returns whether the manager generation represented by this key is still retained.
    pub fn is_alive(&self) -> bool {
        self.0.strong_count() > 0
    }
}

impl std::fmt::Debug for McpResourceClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpResourceClient")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for McpResourceClientGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpResourceClientGeneration")
            .finish_non_exhaustive()
    }
}

impl McpResourceClient {
    /// Creates a resource client backed by the session's replaceable MCP manager.
    pub fn new(manager: Arc<ArcSwap<McpConnectionManager>>) -> Self {
        Self { manager }
    }

    /// Captures the currently published manager for a generation-consistent operation.
    pub fn capture_generation(&self) -> McpResourceClientGeneration {
        McpResourceClientGeneration {
            manager: self.manager.load_full(),
        }
    }

    /// Returns an identity that changes whenever the published manager changes.
    pub fn cache_key(&self) -> McpResourceClientCacheKey {
        self.capture_generation().cache_key()
    }

    /// Returns whether the current manager contains the named server.
    ///
    /// This does not wait for server startup or imply that startup succeeded.
    pub async fn has_server(&self, server: &str) -> bool {
        self.capture_generation().has_server(server)
    }

    /// Lists one resource page from the named server.
    pub async fn list_resources(
        &self,
        server: &str,
        cursor: Option<String>,
    ) -> Result<McpResourcePage> {
        let params =
            cursor.map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
        self.capture_generation()
            .list_resources_with_params(server, params)
            .await
    }

    /// Reads one resource using the manager generation current at call start.
    pub async fn read_resource(&self, server: &str, uri: &str) -> Result<McpResourceReadResult> {
        self.capture_generation().read_resource(server, uri).await
    }
}

impl McpResourceClientGeneration {
    /// Returns the identity of this exact manager generation.
    pub fn cache_key(&self) -> McpResourceClientCacheKey {
        McpResourceClientCacheKey(Arc::downgrade(&self.manager))
    }

    /// Returns whether this generation contains the named server.
    pub fn has_server(&self, server: &str) -> bool {
        self.manager.contains_server(server)
    }

    /// Lists one resource page through this exact manager generation.
    pub async fn list_resources(
        &self,
        server: &str,
        cursor: Option<String>,
    ) -> Result<McpResourcePage> {
        let params =
            cursor.map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
        self.list_resources_with_params(server, params).await
    }

    async fn list_resources_with_params(
        &self,
        server: &str,
        params: Option<PaginatedRequestParams>,
    ) -> Result<McpResourcePage> {
        let result = self.manager.list_resources(server, params).await?;
        let resources = result
            .resources
            .into_iter()
            .map(resource_from_rmcp)
            .collect::<Result<Vec<_>>>()?;
        Ok(McpResourcePage {
            resources,
            next_cursor: result.next_cursor,
        })
    }

    /// Reads one resource through this exact manager generation.
    pub async fn read_resource(&self, server: &str, uri: &str) -> Result<McpResourceReadResult> {
        let result = self
            .manager
            .read_resource(server, ReadResourceRequestParams::new(uri.to_string()))
            .await?;
        let contents = result
            .contents
            .into_iter()
            .map(resource_content_from_rmcp)
            .collect::<Result<Vec<_>>>()?;
        Ok(McpResourceReadResult { contents })
    }
}

fn resource_from_rmcp(resource: rmcp::model::Resource) -> Result<Resource> {
    let value = serde_json::to_value(resource).context("failed to serialize MCP resource")?;
    Resource::from_mcp_value(value).context("failed to convert MCP resource")
}

fn resource_content_from_rmcp(content: rmcp::model::ResourceContents) -> Result<ResourceContent> {
    let value =
        serde_json::to_value(content).context("failed to serialize MCP resource content")?;
    serde_json::from_value(value).context("failed to convert MCP resource content")
}
