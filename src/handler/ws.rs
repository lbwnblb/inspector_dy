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
    let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
    if method == "Runtime.consoleAPICalled" {
        if let Some(args) = v["params"]["args"].as_array() {
            // 用 as_str() 获取原始字符串，避免 to_string() 加 JSON 引号
            let parts: Vec<String> = args.iter().map(|a| {
                a.get("value")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .or_else(|| {
                        a.get("value").map(|v| v.to_string())
                    })
                    .or_else(|| {
                        a.get("description")
                            .and_then(|d| d.as_str().map(|s| s.to_string()))
                    })
                    .unwrap_or_default()
            }).collect();
            let joined = parts.join(" ");

            // 从 "[TAG] ..." 格式中提取 JSON（对象或数组）
            let extract_json = |s: &str| -> Option<serde_json::Value> {
                // 找第一个 '{' 或 '['
                let obj_pos = s.find('{');
                let arr_pos = s.find('[');
                let start = match (obj_pos, arr_pos) {
                    (Some(o), Some(a)) => o.min(a),
                    (Some(o), None) => o,
                    (None, Some(a)) => a,
                    _ => return None,
                };
                // 跳过 tag 里的 '['（如 "[NEW_MSG_LIST]"）
                // 如果 '[' 在 ']' 前面且看起来像 tag，用后面的
                if s[start..].starts_with('[') {
                    if let Some(close) = s[start..].find(']') {
                        let after = start + close + 1;
                        if let Some(next) = s[after..].find(|c: char| c == '{' || c == '[') {
                            return serde_json::from_str(&s[after + next..]).ok();
                        }
                    }
                }
                serde_json::from_str(&s[start..]).ok()
            };

            // ---- 消息列表（扫描模式） ----
            if joined.contains("[NEW_MSG_LIST]") {
                if let Some(arr) = extract_json(&joined) {
                    if let Some(msgs) = arr.as_array() {
                        println!("\n╔══════════════════════════════════════");
                        println!("║ 📋 消息列表更新（最近 {} 条）", msgs.len());
                        println!("╟──────────────────────────────────────");
                        for (i, msg) in msgs.iter().enumerate() {
                            let text = msg["text"].as_str().unwrap_or("(空)");
                            let role = msg["role"].as_str().unwrap_or("?");
                            let (icon, label) = match role {
                                "buyer" => ("👤", "买家"),
                                "agent" => ("🤖", "客服"),
                                _ => ("❓", "未知"),
                            };
                            // 多行消息缩进显示
                            let lines: Vec<&str> = text.lines().collect();
                            if lines.len() == 1 {
                                println!("║ {:>2}. {} [{}] {}", i + 1, icon, label, text);
                            } else {
                                println!("║ {:>2}. {} [{}] {}", i + 1, icon, label, lines[0]);
                                for line in &lines[1..] {
                                    println!("║              {}", line);
                                }
                            }
                        }
                        println!("╚══════════════════════════════════════\n> ");
                        return;
                    }
                }
            }

            // ---- 其他 HOOK 日志 ----
            if joined.contains("[HOOK") || joined.contains("[MAIN]") {
                println!("[hook] {}", joined);
            }
        }
    }
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
