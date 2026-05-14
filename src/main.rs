mod utils;

// src/main.rs
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use crate::utils::js_utils::JS_CODE;

#[derive(Deserialize)]
struct DebugTarget {
    #[serde(rename = "webSocketDebuggerUrl")]
    ws_url: String,
    title: String,
}

#[derive(Serialize)]
struct CdpCommand {
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 获取调试目标
    let targets: Vec<DebugTarget> = reqwest::get("http://localhost:9229/json")
        .await?
        .json()
        .await?;

    let target = &targets[0];
    println!("连接到: {} ({})", target.title, target.ws_url);

    let (mut ws, _) = connect_async(&target.ws_url).await?;
    println!("WebSocket 已连接");

    let cmd = CdpCommand {
        id: 1,
        method: "Debugger.enable".into(),
        params: None,
    };
    let msg = serde_json::to_string(&cmd)?;
    ws.send(Message::Text(msg.into())).await?;
    println!("调试器已启用");

    if let Some(Ok(msg)) = ws.next().await {
        println!("响应: {}", msg);
    }

    let cmd = CdpCommand {
        id: 2,
        method: "Runtime.evaluate".into(),
        params: Some(serde_json::json!({ "expression": JS_CODE })),
    };
    let msg = serde_json::to_string(&cmd)?;
    ws.send(Message::Text(msg.into())).await?;
    println!("JS 代码已注入");

    if let Some(Ok(msg)) = ws.next().await {
        println!("响应: {}", msg);
    }

    Ok(())
}