mod cdp;
mod handler;
mod utils;

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

const CDP_PORT: u16 = 9229;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 连接 CDP 并初始化（启用 Runtime + 注入 Hook）
    println!("正在连接调试端口 {}...", CDP_PORT);
    let ws = cdp::connect(CDP_PORT).await?;
    let (ws_write, ws_read) = cdp::init(ws).await?;

    // 2. 打印帮助
    println!();
    println!("============================================");
    println!("  交互式命令（输入后回车执行）：");
    println!("    send <消息内容>   — 发送消息");
    println!("    inspect           — 查看页面 DOM 结构");
    println!("    rehook            — 重新注入 Hook");
    println!("    monitor           — 检查/重启消息监听");
    println!("    quit / exit       — 退出");
    println!("============================================");
    println!();

    // 3. 启动 stdin → WebSocket 双向通信
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<String>(16);
    let next_id = Arc::new(AtomicU64::new(10));

    let stdin_task = handler::stdin::spawn(cmd_tx, next_id);
    let ws_task = handler::ws::spawn(ws_write, ws_read, cmd_rx);

    // 任一任务结束即退出
    tokio::select! {
        _ = stdin_task => {}
        _ = ws_task => {}
    }

    println!("程序结束");
    Ok(())
}
