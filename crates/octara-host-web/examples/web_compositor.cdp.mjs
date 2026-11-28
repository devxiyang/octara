// Build and serve the fixture, then run with Node 22+ and a Headless Chrome endpoint.
// cargo build --target wasm32-unknown-unknown -p octara-host-web --example web_compositor
// wasm-bindgen --target web --out-dir <fixture-dir> <target>/wasm32-unknown-unknown/debug/examples/web_compositor.wasm
// Copy web_compositor.html into <fixture-dir> and serve it over HTTP.
// node web_compositor.cdp.mjs http://localhost:9222 http://localhost:8000/web_compositor.html

import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";

const [chrome, fixture] = process.argv.slice(2);
assert(chrome && fixture, "Provide the Chrome debugging endpoint and the fixture URL");
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
    const { result, exceptionDetails } = await cdp("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    assert(!exceptionDetails, exceptionDetails?.exception?.description ?? exceptionDetails?.text);
    return result.value;
}

async function pixels(points) {
    await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
    const { data } = await cdp("Page.captureScreenshot", { format: "png" });
    return evaluate(`(async () => {
        const image = new Image();
        image.src = "data:image/png;base64,${data}";
        await image.decode();
        const screenshot = document.createElement("canvas");
        screenshot.width = image.width;
        screenshot.height = image.height;
        const context = screenshot.getContext("2d");
        context.drawImage(image, 0, 0);
        return ${JSON.stringify(points)}.map(([x, y]) => Array.from(context.getImageData(x, y, 1, 1).data).slice(0, 3));
    })()`);
}

function near(actual, expected) {
    assert(actual.every((channel, i) => Math.abs(channel - expected[i]) <= 3), `${actual} differs from ${expected}`);
}

try {
    await ready;
    await cdp("Page.bringToFront");
    await cdp("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "no-preference" }] });
    await cdp("Emulation.setDeviceMetricsOverride", { width: 640, height: 480, deviceScaleFactor: 1, mobile: false });
    await cdp("Page.navigate", { url: fixture });
    for (let attempt = 0; ; attempt++) {
        const result = await evaluate("document.body?.dataset.result");
        if (result === "ready") break;
        assert(result !== "failed" && attempt < 100, await evaluate("document.querySelector('output')?.value ?? document.title"));
        await delay(50);
    }

    const [left, right, sibling, corner, shadow] = await pixels([[68, 66], [170, 68], [100, 83], [33, 41], [121, 162]]);
    near(left, [255, 64, 64]);
    near(right, [64, 64, 255]);
    near(sibling, [0, 255, 0]);
    near(corner, [255, 255, 255]);
    assert(shadow[0] < 250 && shadow.every(channel => channel === shadow[0]), `Missing neutral shadow: ${shadow}`);

    await evaluate("compositor.fixture.show_popup(true)");
    near((await pixels([[114, 100]]))[0], [255, 0, 255]);
    assert.equal(await evaluate("document.elementFromPoint(114, 100).dataset.octaraId"), "popup");
    await evaluate("compositor.fixture.show_popup(false)");
    near((await pixels([[114, 100]]))[0], [0, 255, 0]);

    const point = await evaluate(`(() => {
        const { fixture, view } = compositor;
        fixture.rotate_card(42);
        const card = view("card");
        const css = getComputedStyle(card);
        const [x, y, z] = css.transformOrigin.split(" ").map(parseFloat);
        const point = new DOMMatrix(css.transform).transformPoint(new DOMPoint(45 - x, 18 - y, -z));
        return [Math.floor(card.offsetLeft + x + point.x / point.w), Math.floor(card.offsetTop + y + point.y / point.w)];
    })()`);
    near((await pixels([point]))[0], [255, 64, 64]);

    // The producer can change its GL content independently of Octara frames.
    await evaluate("compositor.redraw(true)");
    near((await pixels([point]))[0], [64, 255, 64]);

    assert(await evaluate(`(() => {
        const { fixture, canvas, view } = compositor;
        fixture.show_canvas(false);
        if (canvas.isConnected || canvas.parentNode.parentNode) return false;
        fixture.show_canvas(true);
        if (view("external").querySelector("canvas") !== canvas) return false;
        fixture.unmount();
        if (canvas.isConnected || canvas.parentNode.parentNode) return false;
        fixture.unregister();
        return canvas.parentNode === null && canvas.getAttribute("style") === "display: inline;";
    })()`), "The external canvas did not retire, remount, or relinquish its DOM styling");
    console.log("Headless Chrome compositor fixture passed.");
} finally {
    socket.close();
    await fetch(new URL("/json/close/" + id, chrome));
}
