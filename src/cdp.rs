use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{
    connect_async,
    tungstenite::Message,
    MaybeTlsStream, WebSocketStream,
};
use tokio::net::TcpStream;

use crate::utils::js_utils;

// ────────────────── 类型定义 ──────────────────

#[derive(Deserialize)]
pub struct DebugTarget {
    #[serde(rename = "webSocketDebuggerUrl")]
    pub ws_url: String,
    pub title: String,
}

#[derive(Serialize)]
pub struct CdpCommand {
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl CdpCommand {
    /// 构造 Runtime.evaluate 命令
    pub fn evaluate(id: u64, expression: &str) -> Self {
        Self {
            id,
            method: "Runtime.evaluate".into(),
            params: Some(serde_json::json!({
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true
            })),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

// ────────────────── 连接与初始化 ──────────────────

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 连接到 CDP 调试端口，返回 WebSocket 流
pub async fn connect(port: u16) -> anyhow::Result<WsStream> {
    let url = format!("http://localhost:{}/json", port);
    let targets: Vec<DebugTarget> = reqwest::get(&url).await?.json().await?;

    if targets.is_empty() {
        anyhow::bail!("未找到调试目标，请确认 inspector 已开启");
    }

    let target = &targets[0];
    println!("连接到: {} ({})", target.title, target.ws_url);

    let (ws, _) = connect_async(&target.ws_url).await?;
    println!("WebSocket 已连接");
    Ok(ws)
}

/// 启用 Runtime 并注入 Hook，返回拆分后的 (write, read)
pub async fn init(ws: WsStream) -> anyhow::Result<(
    futures_util::stream::SplitSink<WsStream, Message>,
    futures_util::stream::SplitStream<WsStream>,
)> {
    let (mut write, mut read) = ws.split();

    // 启用 Runtime
    let cmd = CdpCommand {
        id: 1,
        method: "Runtime.enable".into(),
        params: None,
    };
    write.send(Message::Text(cmd.to_json().into())).await?;
    wait_for_id(&mut read, 1).await?;
    println!("Runtime 已启用");

    // 注入 Hook
    let cmd = CdpCommand::evaluate(2, js_utils::JS_INJECT_HOOK);
    write.send(Message::Text(cmd.to_json().into())).await?;
    let resp = wait_for_id(&mut read, 2).await?;
    let result_val = resp
        .pointer("/result/result/value")
        .and_then(|v| v.as_str())
        .unwrap_or("(no value)");
    println!("Hook 注入结果: {}", result_val);

    Ok((write, read))
}

/// 等待指定 id 的 CDP 响应，跳过事件推送
async fn wait_for_id(
    read: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
    expected_id: u64,
) -> anyhow::Result<serde_json::Value> {
    loop {
        match read.next().await {
            Some(Ok(Message::Text(text))) => {
                let v: serde_json::Value = serde_json::from_str(&text)?;
                if v.get("id").and_then(|i| i.as_u64()) == Some(expected_id) {
                    return Ok(v);
                }
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => anyhow::bail!("WebSocket 错误: {}", e),
            None => anyhow::bail!("WebSocket 连接已关闭"),
        }
    }
}
