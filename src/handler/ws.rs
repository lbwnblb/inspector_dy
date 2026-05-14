use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tokio::net::TcpStream;

type WsSink = futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
type WsSource = futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

/// 启动 WebSocket 读写任务
pub fn spawn(
    mut ws_write: WsSink,
    mut ws_read: WsSource,
    mut cmd_rx: mpsc::Receiver<String>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                // stdin 发来的命令 → 写入 WebSocket
                Some(json) = cmd_rx.recv() => {
                    if let Err(e) = ws_write.send(Message::Text(json.into())).await {
                        eprintln!("WebSocket 发送失败: {}", e);
                        break;
                    }
                }
                // 从 WebSocket 读取消息
                msg = ws_read.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => handle_message(&text),
                        Some(Err(e)) => {
                            eprintln!("WebSocket 错误: {}", e);
                            break;
                        }
                        None => {
                            println!("WebSocket 连接已关闭");
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    })
}

/// 处理单条 WebSocket 消息
fn handle_message(text: &str) {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };

    // 有 id → 命令响应
    if v.get("id").is_some() {
        let display = format_response(&v);
        println!("\n[结果] {}\n> ", display);
        return;
    }

    // 无 id → 事件推送
    // [console] 打印已关闭，如需开启取消下方注释
    // let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
    // if method == "Runtime.consoleAPICalled" {
    //     if let Some(args) = v["params"]["args"].as_array() {
    //         let parts: Vec<String> = args.iter().map(|a| {
    //             a.get("value")
    //                 .map(|v| v.to_string())
    //                 .unwrap_or_else(|| {
    //                     a.get("description")
    //                         .map(|d| d.to_string())
    //                         .unwrap_or_default()
    //                 })
    //         }).collect();
    //         println!("[console] {}", parts.join(" "));
    //     }
    // }
}

/// 格式化 CDP 响应为可读字符串
fn format_response(v: &serde_json::Value) -> String {
    if let Some(val) = v.pointer("/result/result/value") {
        if let Some(s) = val.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                return serde_json::to_string_pretty(&parsed).unwrap_or(s.to_string());
            }
            return s.to_string();
        }
        return val.to_string();
    }

    if let Some(err) = v.pointer("/result/exceptionDetails") {
        return format!(
            "[JS 异常] {}",
            serde_json::to_string_pretty(err).unwrap_or_default()
        );
    }

    serde_json::to_string_pretty(v).unwrap_or_default()
}
