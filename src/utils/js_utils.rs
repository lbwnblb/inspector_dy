pub const JS_CODE: &str = r#"
    (async () => {
        const Module = process.mainModule.constructor;
        const { webContents } = Module._load('electron');

        const allWC = webContents.getAllWebContents();
        const imWC = allWC.find(wc => wc.getURL().includes('im.jinritemai.com'));
        if (!imWC) return JSON.stringify({ error: '未找到 IM webview' });

        const result = await imWC.executeJavaScript(`
            (async function() {
                const info = {};
                const describeKeys = (obj, depth) => {
                    if (!obj) return null;
                    if (typeof obj === 'function') return 'fn(' + obj.length + ')';
                    if (typeof obj !== 'object') return typeof obj;
                    if (depth <= 0) return '{...}';
                    const r = {};
                    Object.keys(obj).slice(0, 40).forEach(k => {
                        try { r[k] = describeKeys(obj[k], depth - 1); } catch(e) { r[k] = 'err'; }
                    });
                    return r;
                };

                // 1. webviewBridge.getSDKClient
                try {
                    const client = await window.webviewBridge.getSDKClient();
                    if (client) {
                        const keys = Object.keys(client);
                        info.sdkClientKeys = keys.slice(0, 50);
                        const sendKeys = keys.filter(k => {
                            const l = k.toLowerCase();
                            return l.includes('send') || l.includes('msg') || l.includes('message')
                                || l.includes('chat') || l.includes('conv') || l.includes('talk');
                        });
                        info.sdkSendKeys = sendKeys;
                        sendKeys.forEach(k => {
                            info['sdk_' + k] = describeKeys(client[k], 2);
                        });

                        // 如果有 prototype 方法
                        const proto = Object.getPrototypeOf(client);
                        if (proto && proto !== Object.prototype) {
                            const protoKeys = Object.getOwnPropertyNames(proto).filter(k => k !== 'constructor');
                            info.sdkProtoKeys = protoKeys.slice(0, 50);
                            const protoSendKeys = protoKeys.filter(k => {
                                const l = k.toLowerCase();
                                return l.includes('send') || l.includes('msg') || l.includes('message');
                            });
                            info.sdkProtoSendKeys = protoSendKeys;
                        }
                    }
                } catch(e) { info.sdkClientError = e.message; }

                // 2. Garfish 子应用
                try {
                    const gar = window.__GARFISH__;
                    if (gar) {
                        const appKeys = Object.keys(gar).slice(0, 20);
                        info.garfishKeys = appKeys;
                        if (gar.apps) {
                            info.garfishApps = Object.keys(gar.apps);
                        }
                        if (gar.appInfos) {
                            info.garfishAppInfos = Object.keys(gar.appInfos);
                        }
                    }
                } catch(e) { info.garfishError = e.message; }

                // 3. __mona_pigeon_event - 可能是消息事件总线
                try {
                    const pe = window.__mona_pigeon_event;
                    if (pe) {
                        info.pigeonEventKeys = Object.keys(pe).slice(0, 30);
                        // 如果有 _events 或 listeners
                        if (pe._events) info.pigeonEventNames = Object.keys(pe._events).slice(0, 30);
                        if (pe.listeners) info.pigeonHasListeners = true;
                    }
                } catch(e) {}

                // 4. webviewBridge.send 的签名
                try {
                    info.bridgeSendStr = window.webviewBridge.send.toString().substring(0, 300);
                } catch(e) {}

                return JSON.stringify(info, null, 2);
            })()
        `);

        return result;
    })()
"#;