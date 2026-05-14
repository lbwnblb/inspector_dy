use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::cdp::CdpCommand;
use crate::utils::js_utils;

/// 启动 stdin 读取任务，解析命令后通过 channel 发送 CDP JSON
pub fn spawn(
    cmd_tx: mpsc::Sender<String>,
    next_id: Arc<AtomicU64>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();

        loop {
            line.clear();
            print!("> ");
            eprint!("");

            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    eprintln!("读取输入错误: {}", e);
                    break;
                }
            }

            let input = line.trim();
            if input.is_empty() {
                continue;
            }

            if input == "quit" || input == "exit" {
                println!("正在退出...");
                break;
            }

            let id = next_id.fetch_add(1, Ordering::SeqCst);

            let json = match parse_command(input, id) {
                Some(j) => j,
                None => continue,
            };

            if cmd_tx.send(json).await.is_err() {
                eprintln!("发送通道已关闭");
                break;
            }
        }
    })
}

/// 解析用户输入，返回序列化后的 CDP JSON
fn parse_command(input: &str, id: u64) -> Option<String> {
    if input == "inspect" {
        let js = js_utils::build_inspect_dom_js();
        let cmd = CdpCommand::evaluate(id, &js);
        println!("[inspect] 正在查询 DOM...");
        Some(cmd.to_json())
    } else if input == "rehook" {
        let cmd = CdpCommand::evaluate(id, js_utils::JS_INJECT_HOOK);
        println!("[rehook] 正在重新注入...");
        Some(cmd.to_json())
    } else if let Some(msg) = input.strip_prefix("send ") {
        let msg = msg.trim();
        if msg.is_empty() {
            println!("用法: send <消息内容>");
            return None;
        }
        let js = js_utils::build_send_message_js(msg);
        let cmd = CdpCommand::evaluate(id, &js);
        println!("[send] 正在发送: {}", msg);
        Some(cmd.to_json())
    } else {
        println!("未知命令: {}", input);
        println!("可用: send <msg> | inspect | rehook | quit");
        None
    }
}
