//! HttpDeck MCP Server：通过 Streamable HTTP 暴露解析与执行能力。
//! AI agent 调用产生的请求会写入工作区 .http-client/history.json，与 GUI 操作共享历史。
//! globals / cookies 通过磁盘持久化，与 GUI 跨进程共享。

use std::sync::Arc;

use clap::Parser;
use httpdeck_lib::{AppState, parser};
use httpdeck_lib::mcp_lock::{self, McpServerInfo};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use serde::Deserialize;
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(name = "httpdeck-mcp", about = "MCP server for HttpDeck")]
struct Args {
    /// 监听端口
    #[arg(long, default_value_t = 18787)]
    port: u16,
}

#[derive(Clone)]
struct McpHandler {
    state: Arc<AppState>,
}

impl McpHandler {
    fn new() -> Self {
        Self {
            state: Arc::new(AppState::default()),
        }
    }
}

// ---- 参数结构体 ----

#[derive(Deserialize, schemars::JsonSchema)]
struct ParseHttpTextParams {
    /// 要解析的 .http 文本内容
    content: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct RunRequestParams {
    /// 工作区根目录路径
    workspace: String,
    /// .http 文件路径
    file: String,
    /// 请求索引（从 0 开始）
    index: usize,
    /// 环境名（可选）
    env: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct RunFileParams {
    workspace: String,
    file: String,
    env: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct RunFolderParams {
    workspace: String,
    folder: String,
    env: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct WorkspaceParams {
    workspace: String,
}

// ---- 工具实现 ----

fn to_json<T: serde::Serialize>(value: &T) -> Result<Json<Value>, ErrorData> {
    serde_json::to_value(value)
        .map(Json)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

fn err<E: ToString>(e: E) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

#[rmcp::tool_router(server_handler)]
impl McpHandler {
    #[rmcp::tool(name = "parse_http_text", description = "解析 .http 文本，返回请求列表、文件变量与解析问题")]
    fn parse_http_text(
        &self,
        Parameters(params): Parameters<ParseHttpTextParams>,
    ) -> Result<Json<Value>, ErrorData> {
        let parsed = parser::parse(&params.content);
        to_json(&parsed.to_output())
    }

    #[rmcp::tool(name = "run_request", description = "执行单个 .http 文件中的第 index 个请求（0 起），结果记入工作区历史")]
    async fn run_request(
        &self,
        Parameters(params): Parameters<RunRequestParams>,
    ) -> Result<Json<Value>, ErrorData> {
        httpdeck_lib::run_single_request(
            &self.state,
            &params.workspace,
            &params.file,
            params.index,
            params.env.as_deref(),
        )
        .await
        .map_err(err)
        .and_then(|r| to_json(&r))
    }

    #[rmcp::tool(name = "run_file", description = "执行整个 .http 文件的全部请求，结果记入工作区历史")]
    async fn run_file(
        &self,
        Parameters(params): Parameters<RunFileParams>,
    ) -> Result<Json<Value>, ErrorData> {
        httpdeck_lib::run_http_file(&self.state, &params.workspace, &params.file, params.env.as_deref())
            .await
            .map_err(err)
            .and_then(|r| to_json(&r))
    }

    #[rmcp::tool(name = "run_folder", description = "递归执行目录下所有 .http / .rest 文件，结果记入工作区历史")]
    async fn run_folder(
        &self,
        Parameters(params): Parameters<RunFolderParams>,
    ) -> Result<Json<Value>, ErrorData> {
        httpdeck_lib::run_http_folder(
            &self.state,
            &params.workspace,
            &params.folder,
            params.env.as_deref(),
        )
        .await
        .map_err(err)
        .and_then(|r| to_json(&r))
    }

    #[rmcp::tool(name = "list_workspaces", description = "列出最近打开过的工作区目录")]
    fn list_workspaces(&self, _params: Parameters<()>) -> Result<Json<Value>, ErrorData> {
        to_json(&httpdeck_lib::recent::list())
    }

    #[rmcp::tool(name = "list_recent_files", description = "列出工作区最近打开的 .http 文件")]
    fn list_recent_files(
        &self,
        Parameters(params): Parameters<WorkspaceParams>,
    ) -> Result<Json<Value>, ErrorData> {
        httpdeck_lib::list_recent_files_for(&params.workspace)
            .map_err(err)
            .and_then(|r| to_json(&r))
    }

    #[rmcp::tool(name = "list_history", description = "列出工作区的请求执行历史（人操作与 MCP 调用共用同一套记录）")]
    fn list_history(
        &self,
        Parameters(params): Parameters<WorkspaceParams>,
    ) -> Result<Json<Value>, ErrorData> {
        httpdeck_lib::list_history_for(&params.workspace)
            .map_err(err)
            .and_then(|r| to_json(&r))
    }

    #[rmcp::tool(name = "get_globals", description = "查看脚本写入的 client.global 变量（与 GUI 跨进程共享）")]
    fn get_globals(
        &self,
        Parameters(params): Parameters<WorkspaceParams>,
    ) -> Result<Json<Value>, ErrorData> {
        httpdeck_lib::get_globals_for(&self.state, &params.workspace)
            .map_err(err)
            .and_then(|r| to_json(&r))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let port = args.port;

    // 写 lock 文件，声明自身正在运行
    let info = McpServerInfo {
        pid: std::process::id(),
        port,
        endpoint: format!("http://127.0.0.1:{port}/mcp"),
        started_at: chrono::Local::now().to_rfc3339(),
    };
    mcp_lock::write_info(&info).map_err(|e| format!("写 lock 文件失败: {e}"))?;
    println!("[httpdeck-mcp] listening on http://127.0.0.1:{port}/mcp");

    // 退出时清理 lock 文件
    let cleanup_pid = info.pid;
    ctrlc::set_handler(move || {
        if std::process::id() == cleanup_pid {
            mcp_lock::clear_info();
        }
        std::process::exit(0);
    })
    .ok();

    let service = StreamableHttpService::new(
        || Ok(McpHandler::new()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
