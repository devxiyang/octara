// Run with Node 22+, a Chrome remote debugging endpoint, and the served
// wasm-bindgen output for dom_transactions.html.
// node dom_modal.cdp.mjs http://localhost:9222 http://localhost:8000/dom_transactions.html

import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";

const [chrome, fixture] = process.argv.slice(2);
assert(chrome && fixture, "Provide the Chrome debugging endpoint and the fixture URL");
const url = new URL(fixture);
url.searchParams.set("modal", "");
const target = await fetch(new URL("/json/new?about:blank", chrome), { method: "PUT" });
assert(target.ok, "Could not open a Chrome target");
const { id, webSocketDebuggerUrl } = await target.json();
const socket = new WebSocket(webSocketDebuggerUrl);
const ready = new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
});
const requests = new Map();
let sequence = 0;
socket.addEventListener("message", ({ data }) => {
    const reply = JSON.parse(data);
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

async function press(key, shiftKey = false) {
    const event = { key, code: key, windowsVirtualKeyCode: { Tab: 9, Escape: 27 }[key], modifiers: shiftKey ? 8 : 0 };
    await cdp("Input.dispatchKeyEvent", { ...event, type: "rawKeyDown" });
    await cdp("Input.dispatchKeyEvent", { ...event, type: "keyUp" });
}

async function focused(expected) {
    assert.equal(await evaluate("activeId()"), expected);
}

try {
    await ready;
    await cdp("Page.bringToFront");
    await cdp("Emulation.setFocusEmulationEnabled", { enabled: true });
    await cdp("Page.navigate", { url: url.href });
    for (let attempt = 0; ; attempt++) {
        const result = await evaluate("document.body?.dataset.result");
        if (result === "modal-ready") break;
        assert(result !== "failed" && attempt < 100, await evaluate("document.querySelector('output')?.value ?? document.title"));
        await delay(50);
    }
    await evaluate(`
        window.view = id => document.querySelector('[data-octara-id=' + id + ']');
        window.activeId = () => document.activeElement.dataset.octaraId || document.activeElement.id;
        window.addField = (parent, id) => Object.assign(parent.appendChild(document.createElement('input')), { id });
        const surface = document.querySelector('#modal-fixture');
        addField(surface, 'page-field');
        addField(surface, 'page-inert').setAttribute('inert', 'authored');
        addField(document.body, 'embedding-field');
        view('opener').focus();
        view('opener').click();
    `);
    await focused("modal-first");
    assert.deepEqual(await evaluate(`[
        view('background').closest('[inert]') !== null,
        view('background-child').hasAttribute('inert'),
        view('outer-modal').getAttribute('aria-modal'),
        view('portalled-stop').closest('[inert]') === null,
        document.querySelector('#page-field').inert,
        document.querySelector('#page-inert').getAttribute('inert'),
    ]`), [true, false, "true", true, true, "authored"]);
    await evaluate("window.lateField = addField(document.querySelector('#modal-fixture'), 'late-field')");
    assert.equal(await evaluate("lateField.inert"), true);
    await evaluate("view('outer-modal').appendChild(lateField)");
    assert.equal(await evaluate("lateField.inert"), false);
    await evaluate("document.querySelector('#modal-fixture').appendChild(lateField)");
    assert.equal(await evaluate("lateField.inert"), true);
    await evaluate(`
        document.querySelector('#page-field').focus();
        view('background-child').focus();
        document.querySelector('#embedding-field').focus();
    `);
    await focused("modal-first");
    await press("Tab", true);
    await focused("portalled-stop");
    await press("Tab");
    await focused("modal-first");
    await press("Tab");
    await focused("inner-opener");
    await evaluate("view('inner-opener').click()");
    await focused("inner-first");
    assert.deepEqual(await evaluate(`[
        view('outer-modal').closest('[inert]') !== null,
        view('portalled-stop').closest('[inert]') !== null,
        view('inner-modal').getAttribute('aria-modal'),
    ]`), [true, true, "true"]);
    await press("Tab", true);
    await focused("inner-last");
    await press("Tab");
    await focused("inner-first");
    await press("Escape");
    await focused("inner-opener");
    assert.equal(await evaluate("view('portalled-stop').closest('[inert]')"), null);
    await press("Tab");
    await focused("native-first");
    await cdp("Input.insertText", { text: "native" });
    assert.equal(await evaluate("document.querySelector('#native-first').value"), "native");
    await press("Tab");
    await focused("native-second");
    await press("Tab");
    await focused("portalled-stop");
    await press("Tab", true);
    await focused("native-second");
    await press("Tab");
    await focused("portalled-stop");
    await evaluate(`const first = document.querySelector('#native-first'); first.tabIndex = 5; first.focus();`);
    await press("Tab", true);
    await focused("portalled-stop");
    await press("Escape");
    await focused("opener");
    assert.deepEqual(await evaluate(`[
        document.querySelector('#page-field').inert,
        document.querySelector('#page-inert').getAttribute('inert'),
        lateField.inert,
    ]`), [false, "authored", false]);

    await evaluate("document.querySelector('#native-opener').focus(); modalFixture.open()");
    await focused("modal-first");
    await press("Escape");
    await focused("native-opener");
    await evaluate("document.querySelector('#embedding-field').focus(); modalFixture.open()");
    await focused("modal-first");
    await press("Escape");
    await focused("embedding-field");
    console.log("Octara modal CDP regression passed");
} finally {
    socket.close();
    await fetch(new URL("/json/close/" + id, chrome));
}
