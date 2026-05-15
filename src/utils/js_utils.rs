/// 注入 Hook：监听 webviewBridge.send + 定时扫描聊天消息列表
///
/// 架构：
/// - 主进程 imWC.on('console-message') 转发 webview console 到主进程
/// - webview 内 MutationObserver 仅作触发器（debounce 500ms）
/// - 触发后用 requestAnimationFrame 等待渲染完成
/// - 然后扫描整个聊天区域的最近 12 条消息
/// - 用计算样式（flex/margin）+ 位置判断买家/客服
/// - 与上次结果 diff，只报告变化
pub const JS_INJECT_HOOK: &str = r#"
    (async () => {
        const Module = process.mainModule.constructor;
        const { webContents } = Module._load('electron');

        const allWC = webContents.getAllWebContents();
        const imWC = allWC.find(wc => wc.getURL().includes('im.jinritemai.com'));
        if (!imWC) return JSON.stringify({ error: '未找到 IM webview' });

        // ★ 主进程：转发 webview console 到主进程 console（CDP 才能收到）
        if (!imWC.__consoleForwarderInstalled) {
            imWC.__consoleForwarderInstalled = true;
            imWC.on('console-message', (event, level, message) => {
                if (message.startsWith('[NEW_MSG') || message.startsWith('[HOOK')) {
                    console.log(message);
                }
            });
            console.log('[MAIN] console forwarder installed');
        }

        // 重置标记
        await imWC.executeJavaScript(`
            if (window.__msgObserver) { window.__msgObserver.disconnect(); window.__msgObserver = null; }
            window.__bridgeHooked = false;
            window.__msgObserverActive = false;
            'reset OK';
        `);

        // 注入 webview 端代码
        await imWC.executeJavaScript(`
            (function() {
                if (window.__bridgeHooked) return;
                window.__bridgeHooked = true;

                // ========== 1. Hook webviewBridge.send ==========
                if (!window.__sendHooked) {
                    const hookBridge = () => {
                        if (!window.webviewBridge || !window.webviewBridge.send) {
                            setTimeout(hookBridge, 500);
                            return;
                        }
                        window.__sendHooked = true;
                        const _orig = window.webviewBridge.send;
                        window.webviewBridge.send = function(...args) {
                            try {
                                const readable = args.map(a => {
                                    if (a instanceof ArrayBuffer || ArrayBuffer.isView(a))
                                        return { _type: 'binary', len: (a.byteLength || a.length) };
                                    try { return JSON.parse(JSON.stringify(a)); } catch(_) { return String(a); }
                                });
                                console.log('[HOOK send] ' + JSON.stringify(readable));
                            } catch(e) {}
                            return _orig.apply(this, args);
                        };
                        console.log('[HOOK] send hooked');
                    };
                    hookBridge();
                }

                // ========== 2. 扫描式消息监听 ==========
                let lastMsgHash = '';
                let debounceTimer = null;

                // 通过计算样式判断元素对齐方向（向上查 depth 层）
                const getAlignment = (el, depth) => {
                    let cur = el;
                    for (let i = 0; i < depth && cur; i++) {
                        try {
                            const st = getComputedStyle(cur);
                            // flexbox 对齐
                            if (st.display === 'flex' || st.display === 'inline-flex') {
                                if (st.flexDirection === 'row-reverse') return 'right';
                                const jc = st.justifyContent;
                                if (jc === 'flex-end' || jc === 'end' || jc === 'right') return 'right';
                                if (jc === 'flex-start' || jc === 'start' || jc === 'left') return 'left';
                            }
                            // margin auto
                            if (st.marginLeft === 'auto' && st.marginRight !== 'auto') return 'right';
                            if (st.marginRight === 'auto' && st.marginLeft !== 'auto') return 'left';
                            // text-align (不太可靠但兜底)
                            if (st.textAlign === 'right' || st.textAlign === 'end') return 'right';
                        } catch(e) {}
                        cur = cur.parentElement;
                    }
                    return null;
                };

                // 清洗单条消息文本
                const cleanLines = (raw) => {
                    return raw.split('\\n').map(l => l.trim()).filter(l => {
                        if (!l) return false;
                        if (/^\\d{1,2}:\\d{2}(:\\d{2})?$/.test(l)) return false;
                        if (/^(昨天|今天|前天|\\d{4}[\\-\\.])/.test(l)) return false;
                        if (/^(已读|未读|已发送|发送中|发送失败)$/.test(l)) return false;
                        if (/^(系统消息|客服.{0,5}接入|用户超时|从历史会话|以上为历史|对方正在输入)/.test(l)) return false;
                        return true;
                    });
                };

                // 扫描聊天容器中所有消息
                const scrapeMessages = (container) => {
                    const containerRect = container.getBoundingClientRect();
                    if (containerRect.width === 0 || containerRect.height === 0) return [];
                    const centerX = containerRect.left + containerRect.width / 2;

                    const messages = [];

                    // 遍历容器的子元素（每个子元素可能是一条消息行或一个时间分隔符）
                    const walkChildren = (parent, depth) => {
                        if (depth > 6) return;
                        for (const child of parent.children) {
                            const rect = child.getBoundingClientRect();
                            if (rect.width === 0 || rect.height === 0) continue;

                            const rawText = child.innerText?.trim();
                            if (!rawText || rawText.length < 1) continue;

                            // 跳过时间分隔行（窄高度、居中、纯时间）
                            if (/^\\d{1,2}:\\d{2}$/.test(rawText)) continue;
                            if (/^(昨天|今天|前天)\\s*\\d{1,2}:\\d{2}$/.test(rawText)) continue;

                            // 跳过系统消息
                            if (/客服.{0,5}接入|用户超时|从历史会话|以上为历史消息|系统关闭/.test(rawText)) continue;

                            // 清洗文本
                            const lines = cleanLines(rawText);
                            if (lines.length === 0) continue;
                            const text = lines.join('\\n');

                            // 判断角色
                            // 1. 计算样式
                            let role = getAlignment(child, 5);
                            // 2. 位置兜底
                            if (!role) {
                                const elCenter = rect.left + rect.width / 2;
                                if (rect.width < containerRect.width * 0.7) {
                                    role = elCenter < centerX ? 'left' : 'right';
                                }
                            }

                            if (!role) {
                                // 再往子元素找：找最窄的含文本子元素判断位置
                                let narrowest = null;
                                for (const sub of child.querySelectorAll('div, span, p')) {
                                    const sr = sub.getBoundingClientRect();
                                    if (sr.width > 0 && sr.width < containerRect.width * 0.6 && sub.innerText?.trim()) {
                                        if (!narrowest || sr.width < narrowest.width) {
                                            narrowest = { el: sub, width: sr.width, left: sr.left };
                                        }
                                    }
                                }
                                if (narrowest) {
                                    const nc = narrowest.left + narrowest.width / 2;
                                    role = nc < centerX ? 'left' : 'right';
                                }
                            }

                            const msgRole = role === 'right' ? 'agent' : 'buyer';

                            // 去掉 agent 消息开头的客服名称行（短且不含数字/标点的行）
                            let finalText = text;
                            if (msgRole === 'agent' && lines.length > 1) {
                                const firstLine = lines[0];
                                if (firstLine.length <= 15 && !/\\d/.test(firstLine) && !/[?？!！。，,.]/.test(firstLine)) {
                                    finalText = lines.slice(1).join('\\n');
                                }
                            }

                            if (!finalText || finalText.length < 1) continue;

                            // 检查是否和上一条重复（嵌套元素可能产生父子两条）
                            const lastMsg = messages[messages.length - 1];
                            if (lastMsg) {
                                if (lastMsg.text === finalText && Math.abs(lastMsg.y - rect.top) < 20) continue;
                                if (lastMsg.text.includes(finalText) || finalText.includes(lastMsg.text)) {
                                    // 保留更短的（更精确）
                                    if (finalText.length < lastMsg.text.length) {
                                        lastMsg.text = finalText;
                                        lastMsg.role = msgRole;
                                    }
                                    continue;
                                }
                            }

                            messages.push({
                                text: finalText,
                                role: msgRole,
                                y: Math.round(rect.top)
                            });
                        }
                    };

                    walkChildren(container, 0);

                    // 按 y 排序
                    messages.sort((a, b) => a.y - b.y);

                    return messages.slice(-12);
                };

                // 启动
                const startMonitor = () => {
                    const container = document.querySelector(
                        '.message-list,' +
                        '.chat-message-list,' +
                        '.im-message-list,' +
                        'div[class*="messageList"],' +
                        'div[class*="MessageList"],' +
                        'div[class*="message-list"],' +
                        'div[class*="chatContent"],' +
                        'div[class*="chat-content"],' +
                        'div[class*="ChatContent"]'
                    );

                    if (!container) {
                        console.log('[HOOK] chat container not found, retry 1s');
                        setTimeout(startMonitor, 1000);
                        return;
                    }

                    window.__msgObserverActive = true;

                    const doScrape = () => {
                        requestAnimationFrame(() => {
                            try {
                                const msgs = scrapeMessages(container);
                                if (msgs.length === 0) return;

                                const hash = msgs.map(m => m.role + ':' + m.text).join('|');
                                if (hash === lastMsgHash) return;
                                lastMsgHash = hash;

                                console.log('[NEW_MSG_LIST] ' + JSON.stringify(msgs));
                            } catch(e) {
                                console.log('[HOOK scrape error] ' + e.message);
                            }
                        });
                    };

                    // 初始扫描
                    setTimeout(doScrape, 500);

                    // MutationObserver 作为触发器
                    window.__msgObserver = new MutationObserver(() => {
                        clearTimeout(debounceTimer);
                        debounceTimer = setTimeout(doScrape, 500);
                    });
                    window.__msgObserver.observe(container, { childList: true, subtree: true });
                    console.log('[HOOK] scrape-based monitor started');
                };

                setTimeout(startMonitor, 1000);
            })();
        `);

        return JSON.stringify({ status: 'hook injected OK (scrape mode)' });
    })()
"#;

/// 生成「发送消息」的 JS 代码
///
/// 流程：主进程找到 IM webview → 在 webview 内定位输入框 → 设置文本 → 点击发送按钮 / 模拟 Enter
pub fn build_send_message_js(message: &str) -> String {
    // JSON 转义，防止消息内容破坏 JS 语法
    let escaped = serde_json::to_string(message).unwrap_or_else(|_| r#""""#.to_string());

    format!(
        r#"
    (async () => {{
        const Module = process.mainModule.constructor;
        const {{ webContents }} = Module._load('electron');

        const allWC = webContents.getAllWebContents();
        const imWC = allWC.find(wc => wc.getURL().includes('im.jinritemai.com'));
        if (!imWC) return JSON.stringify({{ error: '未找到 IM webview' }});

        const result = await imWC.executeJavaScript(`(async function() {{
            const msg = {escaped};

            // ---- 策略1: contenteditable 富文本编辑器 (Draft.js 等) ----
            const editor = document.querySelector(
                '[contenteditable="true"],' +
                '.public-DraftEditor-content,' +
                '.DraftEditor-root [contenteditable],' +
                'div[data-testid="message-input"],' +
                '.chat-input [contenteditable],' +
                '.im-editor [contenteditable],' +
                '.msg-input [contenteditable]'
            );

            if (editor) {{
                editor.focus();
                editor.innerHTML = '';

                // execCommand 能触发 Draft.js 等框架的内部状态更新
                document.execCommand('insertText', false, msg);

                // 兜底：execCommand 不生效时手动写入
                if (!editor.innerText.trim()) {{
                    editor.innerText = msg;
                    editor.dispatchEvent(new Event('input', {{ bubbles: true }}));
                    editor.dispatchEvent(new Event('change', {{ bubbles: true }}));
                }}

                await new Promise(r => setTimeout(r, 300));

                // 查找发送按钮
                const sendBtn = document.querySelector(
                    'button[data-testid="send-button"],' +
                    'button.send-btn,' +
                    'button.chat-send,' +
                    '.im-send-btn,' +
                    '.send-message-btn,' +
                    'div[class*="send"] button,' +
                    'button[class*="send"],' +
                    'div[class*="Send"] button,' +
                    'button[class*="Send"],' +
                    'span[class*="send"],' +
                    'span[class*="Send"]'
                );

                if (sendBtn) {{
                    sendBtn.click();
                    return JSON.stringify({{ status: 'ok', method: 'button', msg }});
                }}

                // 兜底：模拟 Enter 键
                editor.dispatchEvent(new KeyboardEvent('keydown', {{
                    key: 'Enter', code: 'Enter', keyCode: 13, which: 13,
                    bubbles: true, cancelable: true
                }}));
                return JSON.stringify({{ status: 'ok', method: 'enter', msg }});
            }}

            // ---- 策略2: textarea / input ----
            const textarea = document.querySelector(
                'textarea[class*="input"],' +
                'textarea[class*="chat"],' +
                'textarea[class*="msg"],' +
                'input[class*="chat"],' +
                '.chat-input textarea,' +
                '.msg-input textarea'
            );

            if (textarea) {{
                const nativeSetter = Object.getOwnPropertyDescriptor(
                    window.HTMLTextAreaElement.prototype, 'value'
                )?.set || Object.getOwnPropertyDescriptor(
                    window.HTMLInputElement.prototype, 'value'
                )?.set;

                if (nativeSetter) {{
                    nativeSetter.call(textarea, msg);
                }} else {{
                    textarea.value = msg;
                }}

                textarea.dispatchEvent(new Event('input', {{ bubbles: true }}));
                textarea.dispatchEvent(new Event('change', {{ bubbles: true }}));

                await new Promise(r => setTimeout(r, 300));

                const sendBtn = document.querySelector(
                    'button[class*="send"],' +
                    'button[class*="Send"]'
                );
                if (sendBtn) {{
                    sendBtn.click();
                    return JSON.stringify({{ status: 'ok', method: 'textarea_button', msg }});
                }}

                textarea.dispatchEvent(new KeyboardEvent('keydown', {{
                    key: 'Enter', code: 'Enter', keyCode: 13, which: 13,
                    bubbles: true, cancelable: true
                }}));
                return JSON.stringify({{ status: 'ok', method: 'textarea_enter', msg }});
            }}

            return JSON.stringify({{ error: '未找到输入框', hint: '请检查页面DOM' }});
        }})()`);

        return result;
    }})()
"#
    )
}

/// 生成「查询页面 DOM 结构」的调试 JS，用于帮助定位正确的选择器
pub fn build_inspect_dom_js() -> String {
    r#"
    (async () => {
        const Module = process.mainModule.constructor;
        const { webContents } = Module._load('electron');

        const allWC = webContents.getAllWebContents();
        const imWC = allWC.find(wc => wc.getURL().includes('im.jinritemai.com'));
        if (!imWC) return JSON.stringify({ error: '未找到 IM webview' });

        const result = await imWC.executeJavaScript(`(function() {
            const info = {
                url: location.href,
                editables: [],
                textareas: [],
                buttons: []
            };

            document.querySelectorAll('[contenteditable="true"]').forEach(el => {
                info.editables.push({
                    tag: el.tagName,
                    className: el.className.substring(0, 200),
                    id: el.id,
                    parent: el.parentElement?.className?.substring(0, 100) || ''
                });
            });

            document.querySelectorAll('textarea, input[type="text"]').forEach(el => {
                info.textareas.push({
                    tag: el.tagName,
                    className: el.className.substring(0, 200),
                    id: el.id,
                    placeholder: el.placeholder || ''
                });
            });

            document.querySelectorAll('button, [role="button"]').forEach(el => {
                const text = el.innerText?.trim().substring(0, 50) || '';
                const cls = el.className?.substring(0, 200) || '';
                if (text.match(/发送|send/i) || cls.match(/send/i)) {
                    info.buttons.push({
                        tag: el.tagName,
                        className: cls,
                        text: text,
                        id: el.id
                    });
                }
            });

            return JSON.stringify(info, null, 2);
        })()`);

        return result;
    })()
"#.to_string()
}

/// 生成「检查/重启消息监听」的 JS 代码
pub fn build_monitor_js() -> String {
    r#"
    (async () => {
        const Module = process.mainModule.constructor;
        const { webContents } = Module._load('electron');

        const allWC = webContents.getAllWebContents();
        const imWC = allWC.find(wc => wc.getURL().includes('im.jinritemai.com'));
        if (!imWC) return JSON.stringify({ error: '未找到 IM webview' });

        const result = await imWC.executeJavaScript(`(function() {
            const status = {
                bridgeHooked: !!window.__bridgeHooked,
                observerActive: !!window.__msgObserverActive,
                fetchHooked: !!window.__fetchHooked
            };

            // 如果 observer 没在运行，重置标志让 rehook 可以重新注入
            if (!window.__msgObserverActive) {
                window.__bridgeHooked = false;
                status.needRehook = true;
                status.hint = '请执行 rehook 重新注入（含消息监听）';
            }

            return JSON.stringify(status, null, 2);
        })()`);

        return result;
    })()
"#.to_string()
}
