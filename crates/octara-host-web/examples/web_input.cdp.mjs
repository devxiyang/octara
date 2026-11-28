// Node 22+ and a Headless Chrome debugging endpoint are required.
// node web_input.cdp.mjs http://localhost:9222 http://localhost:8000/web_input.html

import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";

const [endpoint, fixture] = process.argv.slice(2);
const url = new URL(fixture);
url.searchParams.set("trusted-drag", "");
const response = await fetch(new URL("/json/new?about:blank", endpoint), { method: "PUT" });
assert(response.ok, "Could not open a Chrome target");
const { id, webSocketDebuggerUrl } = await response.json();
const socket = new WebSocket(webSocketDebuggerUrl);
const ready = new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
});
const requests = new Map();
let sequence = 0;
let intercepted;
socket.addEventListener("message", ({ data }) => {
    const reply = JSON.parse(data);
    if (reply.method === "Input.dragIntercepted") intercepted = reply.params.data;
    requests.get(reply.id)?.(reply);
});

async function cdp(method, params = {}) {
    const id = ++sequence;
    try {
        const reply = await new Promise((resolve, reject) => {
            const timer = setTimeout(() => reject(new Error(method + " timed out")), 5000);
            requests.set(id, reply => {
                clearTimeout(timer);
                resolve(reply);
            });
            socket.send(JSON.stringify({ id, method, params }));
        });
        assert(!reply.error, reply.error?.message);
        return reply.result;
    } finally {
        requests.delete(id);
    }
}

async function evaluate(expression) {
    const { result, exceptionDetails } = await cdp("Runtime.evaluate", { expression, returnByValue: true });
    assert(!exceptionDetails, exceptionDetails?.exception?.description ?? exceptionDetails?.text);
    return result.value;
}

async function until(expression) {
    for (let attempt = 0; attempt < 120; attempt++) {
        const result = await evaluate(expression);
        if (result) return result;
        if (await evaluate("document.body?.dataset.result") === "failed") break;
        await delay(50);
    }
    assert.fail(await evaluate("document.querySelector('#output')?.textContent") || "The browser fixture did not finish");
}

try {
    await ready;
    await cdp("Page.bringToFront");
    await cdp("Emulation.setFocusEmulationEnabled", { enabled: true });
    await cdp("Input.setInterceptDrags", { enabled: true });
    await cdp("Page.navigate", { url: url.href });
    const { x, y } = await until("window.octaraDragStart");
    await cdp("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await cdp("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1 });
    await cdp("Input.dispatchMouseEvent", { type: "mouseMoved", x: x + 24, y, button: "left", buttons: 1 });
    await until("document.body.dataset.result === 'passed'");
    assert(intercepted?.items.some(item => item.mimeType === "text/plain" && item.data === "Drag test text"), "Chrome did not start a drag with the payload");
    await cdp("Input.dispatchDragEvent", { type: "dragCancel", x: x + 24, y, data: intercepted });
    console.log("Browser input and trusted Drag-Out passed");
} finally {
    socket.close();
    await fetch(new URL("/json/close/" + id, endpoint));
}
