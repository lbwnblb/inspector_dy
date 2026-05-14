/// 注入 Hook：监听 webviewBridge.send 并准备发送能力
pub const JS_INJECT_HOOK: &str = r#"
    (async () => {
        const Module = process.mainModule.constructor;
        const { webContents } = Module._load('electron');

        const allWC = webContents.getAllWebContents();
        const imWC = allWC.find(wc => wc.getURL().includes('im.jinritemai.com'));
        if (!imWC) return JSON.stringify({ error: '未找到 IM webview' });

        await imWC.executeJavaScript(`
            (function() {
                if (window.__bridgeHooked) {
                    console.log('[HOOK] already hooked, skip');
                    return;
                }
                window.__bridgeHooked = true;

                const hookBridge = () => {
                    if (!window.webviewBridge || !window.webviewBridge.send) {
                        console.log('[HOOK] webviewBridge.send not ready, retry in 500ms...');
                        setTimeout(hookBridge, 500);
                        return;
                    }

                    const _origSend = window.webviewBridge.send;
                    window.webviewBridge.send = function(...args) {
                        try {
                            const readable = args.map(a => {
                                if (a instanceof ArrayBuffer || ArrayBuffer.isView(a)) {
                                    return { _type: 'binary', bytes: Array.from(new Uint8Array(a instanceof ArrayBuffer ? a : a.buffer)) };
                                }
                                try { return JSON.parse(JSON.stringify(a)); } catch(_) { return String(a); }
                            });
                            console.log('[HOOK send]', JSON.stringify(readable));
                        } catch(e) {
                            console.log('[HOOK send error]', e.message);
                        }
                        return _origSend.apply(this, args);
                    };
                    console.log('[HOOK] webviewBridge.send hooked OK');
                };

                hookBridge();
            })();
        `);

        return JSON.stringify({ status: 'hook injected OK' });
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
