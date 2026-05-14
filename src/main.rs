mod utils;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use crate::utils::js_utils::{JS_CODE};

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

/// 等待指定 id 的响应，跳过所有事件推送（如 scriptParsed）
async fn wait_for_response(
    ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
    expected_id: u64,
) -> anyhow::Result<serde_json::Value> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(text))) => {
                let v: serde_json::Value = serde_json::from_str(&text)?;
                if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                    if id == expected_id {
                        return Ok(v);
                    }
                }
                // 没有 id 或 id 不匹配 → 事件推送，跳过
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => anyhow::bail!("WebSocket 错误: {}", e),
            None => anyhow::bail!("WebSocket 连接已关闭"),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 获取调试目标
    let targets: Vec<DebugTarget> = reqwest::get("http://localhost:9229/json")
        .await?
        .json()
        .await?;

    if targets.is_empty() {
        anyhow::bail!("未找到调试目标，请确认 inspector 已开启");
    }

    let target = &targets[0];
    println!("连接到: {} ({})", target.title, target.ws_url);

    let (mut ws, _) = connect_async(&target.ws_url).await?;
    println!("WebSocket 已连接");

    // 2. 只启用 Runtime（不用 Debugger.enable，避免大量 scriptParsed 洪水）
    let cmd = CdpCommand {
        id: 1,
        method: "Runtime.enable".into(),
        params: None,
    };
    ws.send(Message::Text(serde_json::to_string(&cmd)?.into())).await?;
    let resp = wait_for_response(&mut ws, 1).await?;
    println!("Runtime 已启用: {}", resp);

    // 3. 注入 JS
    let cmd = CdpCommand {
        id: 2,
        method: "Runtime.evaluate".into(),
        params: Some(serde_json::json!({
            "expression": JS_CODE,
            "awaitPromise": true,
            "returnByValue": true
        })),
    };
    ws.send(Message::Text(serde_json::to_string(&cmd)?.into())).await?;
    println!("JS 代码已发送，等待执行结果...");

    let resp = wait_for_response(&mut ws, 2).await?;
    println!("执行结果: {}", serde_json::to_string_pretty(&resp)?);

    // 4. 持续监听主进程 console 输出（Ctrl+C 退出）
    println!("\n========== 开始监听 IPC 日志（Ctrl+C 退出）==========\n");
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(text))) => {
                let v: serde_json::Value = serde_json::from_str(&text)?;
                let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
                if method == "Runtime.consoleAPICalled" {
                    if let Some(args) = v["params"]["args"].as_array() {
                        let parts: Vec<String> = args.iter().map(|a| {
                            a.get("value")
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| {
                                    a.get("description")
                                        .map(|d| d.to_string())
                                        .unwrap_or_default()
                                })
                        }).collect();
                        println!("[console] {}", parts.join(" "));
                    }
                }
            }
            Some(Err(e)) => {
                eprintln!("WebSocket 错误: {}", e);
                break;
            }
            None => {
                println!("连接已关闭");
                break;
            }
            _ => {}
        }
    }

    Ok(())
}