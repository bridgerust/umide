// Dedicated desktop-browser process. The DevTools connection uses inherited pipes,
// so there is no browser debugging port and no connection to existing profiles.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const readline = require('node:readline');
const { spawn } = require('node:child_process');

const base = new URL(process.argv[2]);
const profile = process.argv[3];
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const pending = new Map();
let chrome, proxy, session, nextId = 0, stopping = false;

function browserExecutable() {
    const names = process.platform === 'win32'
        ? ['chrome.exe', 'msedge.exe', 'brave.exe']
        : ['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser', 'microsoft-edge', 'brave-browser'];
    const candidates = [];
    if (process.env.UMIDE_CHROMIUM_PATH) {
        candidates.push(process.env.UMIDE_CHROMIUM_PATH);
    } else {
        for (const dir of (process.env.PATH || '').split(path.delimiter).filter(p => path.isAbsolute(p))) {
            for (const name of names) candidates.push(path.join(dir, name));
        }
        if (process.platform === 'darwin') {
            for (const dir of ['/Applications', path.join(require('node:os').homedir(), 'Applications')]) {
                for (const name of ['Google Chrome', 'Chromium', 'Microsoft Edge', 'Brave Browser']) {
                    candidates.push(path.join(dir, name + '.app', 'Contents/MacOS', name));
                }
            }
        }
        if (process.platform === 'win32') {
            for (const dir of [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA].filter(Boolean)) {
                for (const exe of ['Google/Chrome/Application/chrome.exe', 'Microsoft/Edge/Application/msedge.exe', 'BraveSoftware/Brave-Browser/Application/brave.exe']) {
                    candidates.push(path.join(dir, exe));
                }
            }
        }
    }
    for (const file of candidates.filter(p => path.isAbsolute(p))) {
        try { fs.accessSync(file, fs.constants.X_OK); if (fs.statSync(file).isFile()) return file; } catch {}
    }
    throw Error('Install Chrome, Chromium, Edge or Brave, or set UMIDE_CHROMIUM_PATH to its executable, then run web_start.');
}

function command(method, params = {}, sessionId = session) {
    return new Promise((resolve, reject) => {
        if (!chrome || chrome.exitCode !== null || chrome.stdio[3].destroyed) {
            reject(Error('The test browser has closed. Run web_start again.'));
            return;
        }
        const id = ++nextId;
        const timer = setTimeout(() => {
            pending.delete(id);
            reject(Error('The test browser timed out while running ' + method + '.'));
        }, 8000);
        pending.set(id, { resolve, reject, timer });
        const payload = JSON.stringify({ id, method, params, sessionId }) + '\0';
        chrome.stdio[3].write(payload);
    });
}

function receive(message) {
    if (message.id) {
        const entry = pending.get(message.id);
        if (!entry) return;
        pending.delete(message.id);
        clearTimeout(entry.timer);
        if (message.error) entry.reject(Error(message.error.message));
        else entry.resolve(message.result);
    }
    if (message.method === 'Page.javascriptDialogOpening') {
        command('Page.handleJavaScriptDialog', { accept: false }, message.sessionId).catch(() => {});
    }
}

async function launch() {
    const executable = browserExecutable();
    // All non-preview origins, including other loopback ports and WebSockets,
    // reach an empty local proxy. It never forwards or resolves a request.
    proxy = net.createServer(socket => socket.destroy());
    await new Promise((resolve, reject) => {
        proxy.once('error', reject);
        proxy.listen(0, '127.0.0.1', resolve);
    });
    if (stopping) throw Error('Test session closed.');
    chrome = spawn(executable, [
        '--headless=new', '--remote-debugging-pipe', '--user-data-dir=' + profile,
        '--no-first-run', '--no-default-browser-check', '--disable-extensions',
        '--disable-background-networking', '--disable-component-update', '--disable-sync',
        '--disable-quic', '--force-webrtc-ip-handling-policy=disable_non_proxied_udp',
        '--proxy-server=http://127.0.0.1:' + proxy.address().port,
        '--proxy-bypass-list=<-loopback>;' + base.host,
        '--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1',
        'about:blank',
    ], { stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe'], windowsHide: true });
    const closed = () => {
        for (const { reject, timer } of pending.values()) {
            clearTimeout(timer);
            reject(Error('Chromium could not run in headless mode. Check UMIDE_CHROMIUM_PATH and browser permissions.'));
        }
        pending.clear();
    };
    chrome.on('error', closed);
    chrome.on('exit', closed);
    chrome.stdio[3].on('error', closed);
    let tail = '';
    chrome.stdio[4].setEncoding('utf8').on('data', data => {
        const frames = (tail + data).split('\0');
        tail = frames.pop();
        for (const frame of frames) if (frame) receive(JSON.parse(frame));
    });
    const { targetInfos } = await command('Target.getTargets');
    const target = targetInfos.find(t => t.type === 'page');
    if (!target) throw Error('Chromium did not create a test tab.');
    ({ sessionId: session } = await command('Target.attachToTarget', { targetId: target.targetId, flatten: true }));
    await command('Page.enable');
    await command('Browser.setDownloadBehavior', { behavior: 'deny' });
    await command('Emulation.setDeviceMetricsOverride', { width: 1024, height: 768, deviceScaleFactor: 1, mobile: false });
}

// This function executes in a Chromium isolated world. Only typed arguments
// cross the boundary; controls refer to elements from the last inspection.
function documentAction(action, number, value, origin) {
    if (location.origin !== origin) throw Error('The page left the development server. Use web_open with a local path.');
    if (!document.body || document.readyState === 'loading') {
        if (['inspect', 'wait', 'ready'].includes(action)) return null;
        throw Error('The page is loading. Run web_inspect when it is ready.');
    }
    if (action === 'ready') return true;
    const visible = el => {
        const style = getComputedStyle(el);
        return el.getClientRects().length && style.visibility !== 'hidden' && style.visibility !== 'collapse'
            && style.display !== 'none' && style.opacity !== '0' && !el.closest('[inert]');
    };
    if (action === 'wait') return document.body.innerText.includes(value);
    if (action === 'inspect') {
        const selector = 'a[href],button,input:not([type=hidden]),textarea,select,summary,[role=button],[role=link],[role=checkbox],[role=tab],[contenteditable=true]';
        const nodes = [...document.querySelectorAll(selector)].filter(visible);
        globalThis.umideControls = nodes;
        const { text_offset: textOffset = 0, control_offset: controlOffset = 0 } = value;
        const controls = nodes.slice(controlOffset, controlOffset + 20).map((el, i) => {
            const labelledBy = (el.getAttribute('aria-labelledby') || '').split(/\s+/).filter(Boolean)
                .map(id => document.getElementById(id)?.textContent).filter(Boolean).join(' ');
            const label = el.getAttribute('aria-label') || labelledBy || el.labels?.[0]?.textContent ||
                el.innerText || el.placeholder || el.name || el.title || (el.type === 'submit' && el.value) || '';
            const info = { control: controlOffset + i + 1, kind: el.getAttribute('role') || (el.tagName === 'INPUT' ? el.type : el.localName) };
            if (label) info.label = label.trim().slice(0, 80);
            if (el.matches(':disabled,[aria-disabled=true]')) info.disabled = true;
            if (el.type === 'checkbox' || el.type === 'radio') info.checked = el.checked;
            if (['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName) && el.type !== 'password' && el.type !== 'file') {
                if (el.value) info.value = el.value.slice(0, 80);
            }
            return info;
        });
        const fullText = document.body.innerText;
        const result = { path: (location.pathname + location.search + location.hash).slice(0, 250), title: document.title.slice(0, 120),
            text: fullText.slice(textOffset, textOffset + 1400), controls };
        if (controlOffset + controls.length < nodes.length) result.next_control_offset = controlOffset + controls.length;
        if (textOffset + 1400 < fullText.length) result.next_text_offset = textOffset + 1400;
        return result;
    }
    const el = globalThis.umideControls?.[number - 1];
    if (!el?.isConnected || !visible(el)) throw Error('This control is no longer visible. Run web_inspect for its current number.');
    if (el.matches(':disabled,[aria-disabled=true]')) throw Error('The control is disabled.');
    if (action === 'click') {
        const anchor = el.closest('a[href]');
        if (anchor && (new URL(anchor.href).origin !== origin || anchor.target && anchor.target !== '_self')) {
            throw Error('The link leaves this test tab. Choose a link in the same development server.');
        }
        el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
        const r = el.getBoundingClientRect();
        const x = (Math.max(0, r.left) + Math.min(innerWidth, r.right)) / 2;
        const y = (Math.max(0, r.top) + Math.min(innerHeight, r.bottom)) / 2;
        if (!el.contains(document.elementFromPoint(x, y))) throw Error('The control is covered by another element.');
        return { x, y };
    }
    const field = el.matches('textarea,input:is([type=text],[type=email],[type=search],[type=tel],[type=url],[type=password],[type=number],:not([type]))');
    if ((!field && !el.isContentEditable) || el.readOnly) throw Error('Choose an editable text field with web_inspect.');
    el.focus();
    if (field) el.select();
    else {
        const selection = getSelection(), range = document.createRange();
        range.selectNodeContents(el);
        selection.removeAllRanges();
        selection.addRange(range);
    }
    return true;
}

async function page(action, control, text) {
    const { frameTree } = await command('Page.getFrameTree');
    const { executionContextId } = await command('Page.createIsolatedWorld', { frameId: frameTree.frame.id, worldName: 'umide-browser-test' });
    const { result, exceptionDetails } = await command('Runtime.callFunctionOn', {
        functionDeclaration: documentAction.toString(), executionContextId, returnByValue: true,
        arguments: [action, control ?? null, text ?? '', base.origin].map(value => ({ value })),
    });
    if (exceptionDetails) throw Error((exceptionDetails.exception?.description || exceptionDetails.text).split('\n')[0]);
    return result.value;
}

async function readWhenReady(action = 'inspect', options = {}) {
    const deadline = Date.now() + 5000;
    let lastError;
    do {
        try {
            const result = await page(action, null, options);
            if (result) return result;
        } catch (e) { lastError = e; }
        await sleep(80);
    } while (Date.now() < deadline);
    throw lastError || Error('The page is still loading. Retry web_inspect.');
}

async function perform(name, args) {
    const textBlock = result => ({ type: 'text', text: JSON.stringify(result) });
    if (name === 'web_open') {
        const route = args.path;
        if (typeof route !== 'string' || route.length > 2048 || !route.startsWith('/') || route.startsWith('//') || /[\\\x00-\x1f]/.test(route)) {
            throw Error('Provide a local site path beginning with a single /.');
        }
        const url = new URL(route, base);
        if (url.origin !== base.origin) throw Error('Choose a path on this development server.');
        const nav = await command('Page.navigate', { url: url.href });
        if (nav.errorText) throw Error('Navigation failed: ' + nav.errorText);
    } else if (name === 'web_fill' || name === 'web_click') {
        if (!Number.isSafeInteger(args.control) || args.control < 1) {
            throw Error('Provide a control number from the latest web_inspect.');
        }
        if (name === 'web_fill') {
            if (typeof args.text !== 'string' || args.text.length > 4096) throw Error('Provide up to 4096 characters for the field.');
            await page('fill', args.control);
            if (args.text) await command('Input.insertText', { text: args.text });
            else {
                for (const type of ['keyDown', 'keyUp']) {
                    await command('Input.dispatchKeyEvent', { type, key: 'Backspace', code: 'Backspace', windowsVirtualKeyCode: 8 });
                }
            }
        } else {
            const point = await page('click', args.control);
            for (const type of ['mousePressed', 'mouseReleased']) {
                await command('Input.dispatchMouseEvent', { type, ...point, button: 'left', clickCount: 1 });
            }
        }
        return textBlock(name === 'web_fill' ? { filled: args.control } : { clicked: args.control });
    } else if (name === 'web_wait') {
        if (typeof args.text !== 'string' || !args.text.trim() || args.text.length > 4096) throw Error('Provide visible text to wait for.');
        const until = Date.now() + 5000;
        while (!(await page('wait', null, args.text))) {
            if (Date.now() >= until) throw Error('Expected text did not appear within five seconds. Run web_inspect to read the current page.');
            await sleep(80);
        }
        return textBlock({ observed: args.text.slice(0, 160) });
    } else if (name === 'web_screenshot') {
        await readWhenReady('ready');
        const { data } = await command('Page.captureScreenshot', { format: 'jpeg', quality: 85, captureBeyondViewport: false });
        return { type: 'image', mimeType: 'image/jpeg', data };
    } else if (name === 'web_inspect') {
        for (const offset of [args.control_offset, args.text_offset]) {
            if (offset !== undefined && (!Number.isSafeInteger(offset) || offset < 0)) throw Error('Offsets must be nonnegative integers.');
        }
    } else {
        throw Error('Unknown browser verification action.');
    }
    return textBlock(await readWhenReady('inspect', args));
}

async function stop() {
    if (stopping) return;
    stopping = true;
    proxy?.close();
    if (chrome?.exitCode === null) {
        try {
            await Promise.race([command('Browser.close'), sleep(600)]);
        } catch {}
        if (chrome.exitCode === null) {
            const done = new Promise(resolve => chrome.once('exit', resolve));
            await Promise.race([done, sleep(500)]);
            if (chrome.exitCode === null) chrome.kill('SIGKILL');
        }
    }
    process.exit();
}
const lines = readline.createInterface({ input: process.stdin });
lines.once('close', stop);
process.on('SIGTERM', stop);
process.on('SIGINT', stop);
let started;
(async () => {
    for await (const line of lines) {
        try {
            const { name, args } = JSON.parse(line);
            started ||= launch();
            await started;
            const content = await perform(name, args);
            process.stdout.write(JSON.stringify({ content }) + '\n');
        } catch (e) {
            const closed = !chrome || chrome.exitCode !== null || chrome.signalCode !== null;
            process.stdout.write(JSON.stringify({ error: e.message, closed }) + '\n');
        }
    }
})();
