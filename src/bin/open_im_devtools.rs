//! 专门用于打开 IM 聊天 webview 的 DevTools
//!
//! 飞鸽客服的真正聊天 UI 在嵌套 webview 里（im.jinritemai.com），
//! 外壳 BrowserWindow 的 DevTools 看不到它的网络请求和 DOM。
//! 本工具通过 CDP 找到该 webview 的 webContents 并为其打开 DevTools。
//!
//! 用法: cargo run --bin open_im_devtools

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const CDP_PORT: u16 = 9229;

#[derive(Deserialize)]
struct DebugTarget {
    #[serde(rename = "webSocketDebuggerUrl")]
    ws_url: String,
    title: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("正在连接调试端口 {}...", CDP_PORT);

    // 1. 获取调试目标
    let url = format!("http://localhost:{}/json", CDP_PORT);
    let targets: Vec<DebugTarget> = reqwest::get(&url).await?.json().await?;

    if targets.is_empty() {
        anyhow::bail!("未找到调试目标，请确认 inspector 已开启");
    }

    let target = &targets[0];
    println!("连接到: {} ({})", target.title, target.ws_url);

    // 2. 建立 WebSocket
    let (ws, _) = connect_async(&target.ws_url).await?;
    let (mut write, mut read) = ws.split();

    // 3. 启用 Runtime
    let enable_cmd = serde_json::json!({
        "id": 1,
        "method": "Runtime.enable",
    });
    write
        .send(Message::Text(enable_cmd.to_string().into()))
        .await?;
    wait_for_id(&mut read, 1).await?;
    println!("Runtime 已启用\n");

    // 4. 先列出所有 webContents，让用户看到全貌
    println!("========== 第一步：列出所有 webContents ==========\n");

    let list_js = r#"
    (async () => {
        const Module = process.mainModule.constructor;
        const { webContents } = Module._load('electron');

        const allWC = webContents.getAllWebContents();
        const list = allWC.map(wc => ({
            id: wc.id,
            url: wc.getURL(),
            title: wc.getTitle(),
            type: wc.getType(),
            devToolsOpened: wc.isDevToolsOpened()
        }));

        return JSON.stringify(list, null, 2);
    })()
    "#;

    let list_cmd = serde_json::json!({
        "id": 2,
        "method": "Runtime.evaluate",
        "params": {
            "expression": list_js,
            "awaitPromise": true,
            "returnByValue": true
        }
    });
    write
        .send(Message::Text(list_cmd.to_string().into()))
        .await?;

    let resp = wait_for_id(&mut read, 2).await?;
    if let Some(val) = resp.pointer("/result/result/value").and_then(|v| v.as_str()) {
        if let Ok(arr) = serde_json::from_str::<serde_json::Value>(val) {
            if let Some(items) = arr.as_array() {
                for item in items {
                    let id = item["id"].as_u64().unwrap_or(0);
                    let url = item["url"].as_str().unwrap_or("");
                    let wc_type = item["type"].as_str().unwrap_or("");
                    let title = item["title"].as_str().unwrap_or("");
                    let devtools = item["devToolsOpened"].as_bool().unwrap_or(false);

                    let marker = if url.contains("im.jinritemai.com") {
                        " ★ ← IM 聊天 webview"
                    } else {
                        ""
                    };

                    println!(
                        "  [id={}] type={:<12} devtools={:<5} title={}\n         url={}{}\n",
                        id, wc_type, devtools, title, truncate(url, 120), marker
                    );
                }
            }
        }
    }

    // 5. 打开 IM webview 的 DevTools
    println!("========== 第二步：打开 IM webview DevTools ==========\n");

    let open_js = r#"
    (async () => {
        const Module = process.mainModule.constructor;
        const { webContents } = Module._load('electron');

        const allWC = webContents.getAllWebContents();

        // 按优先级查找 IM webview
        let imWC = allWC.find(wc => wc.getURL().includes('im.jinritemai.com'));

        if (!imWC) {
            // 备选：找 type=webview 且 URL 包含 seller 或 desk
            imWC = allWC.find(wc =>
                wc.getType() === 'webview' &&
                (wc.getURL().includes('seller') || wc.getURL().includes('desk'))
            );
        }

        if (!imWC) {
            return JSON.stringify({
                error: '未找到 IM webview',
                hint: '请确保飞鸽客服已打开聊天窗口',
                available: allWC.map(wc => ({
                    id: wc.id,
                    type: wc.getType(),
                    url: wc.getURL().substring(0, 150)
                }))
            });
        }

        // 打开 DevTools
        if (imWC.isDevToolsOpened()) {
            return JSON.stringify({
                status: 'already_open',
                id: imWC.id,
                url: imWC.getURL(),
                message: 'DevTools 已经打开了'
            });
        }

        imWC.openDevTools({ mode: 'detach' });

        return JSON.stringify({
            status: 'opened',
            id: imWC.id,
            url: imWC.getURL(),
            type: imWC.getType(),
            title: imWC.getTitle(),
            message: 'DevTools 已打开（detach 模式）'
        });
    })()
    "#;

    let open_cmd = serde_json::json!({
        "id": 3,
        "method": "Runtime.evaluate",
        "params": {
            "expression": open_js,
            "awaitPromise": true,
            "returnByValue": true
        }
    });
    write
        .send(Message::Text(open_cmd.to_string().into()))
        .await?;

    let resp = wait_for_id(&mut read, 3).await?;
    if let Some(val) = resp.pointer("/result/result/value").and_then(|v| v.as_str()) {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(val) {
            let pretty = serde_json::to_string_pretty(&parsed).unwrap_or(val.to_string());
            println!("{}", pretty);

            // 根据状态给出提示
            if let Some(status) = parsed["status"].as_str() {
                println!();
                match status {
                    "opened" => {
                        println!("✅ 成功！IM webview 的 DevTools 已在新窗口打开。");
                        println!("   现在你可以在里面查看 Network、DOM、Console 等。");
                    }
                    "already_open" => {
                        println!("ℹ️  DevTools 已经打开了，切换到对应窗口即可。");
                    }
                    _ => {}
                }
            }
            if parsed.get("error").is_some() {
                println!("\n❌ {}", parsed["error"].as_str().unwrap_or("未知错误"));
                if let Some(hint) = parsed["hint"].as_str() {
                    println!("   提示: {}", hint);
                }
            }
        } else {
            println!("{}", val);
        }
    } else if let Some(err) = resp.pointer("/result/exceptionDetails") {
        println!("JS 执行异常:\n{}", serde_json::to_string_pretty(err)?);
    }

    println!("\n程序结束");
    Ok(())
}

/// 等待指定 id 的 CDP 响应
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

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}
