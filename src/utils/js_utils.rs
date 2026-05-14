pub const JS_CODE: &str = r#"
    (async () => {
        const Module = process.mainModule.constructor;
        const { webContents } = Module._load('electron');

        const allWC = webContents.getAllWebContents();
        const imWC = allWC.find(wc => wc.getURL().includes('im.jinritemai.com'));
        if (!imWC) return JSON.stringify({ error: '未找到 IM webview' });

        // 注入 Hook，持续监听，不等待结果
        await imWC.executeJavaScript(`
            (function() {
                // 防止重复注入
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
                            // 尝试把参数解析成可读结构
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

        return JSON.stringify({ status: 'hook injected, waiting for send calls...' });
    })()
"#;