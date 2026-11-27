// Read a bounded response and report the first 16 KiB before transfer completes.
export async function fetchImage(uri, headers, cache, signal, probe) {
    const response = await fetch(uri, {
        headers, cache, signal, redirect: headers.length === 0 ? "follow" : "error",
    });
    if (!response.ok) throw new Error("Image HTTP request failed");
    const limit = 64 * 1024 * 1024;
    if (Number(response.headers.get("Content-Length")) > limit || response.body == null) {
        throw new Error("Invalid image response");
    }
    const reader = response.body.getReader();
    const chunks = [];
    const header = new Uint8Array(16 * 1024);
    let size = 0;
    let found = false;
    let complete = false;
    try {
        for (;;) {
            const { value, done } = await reader.read();
            if (done) {
                complete = true;
                break;
            }
            if (signal.aborted || size + value.length > limit) throw new Error("Image transfer stopped");
            if (!found && size < header.length) {
                header.set(value.subarray(0, header.length - size), size);
                found = probe(header.subarray(0, Math.min(size + value.length, header.length)));
            }
            chunks.push(value);
            size += value.length;
        }
    } finally {
        if (!complete) await reader.cancel().catch(() => {});
        reader.releaseLock();
    }
    const directives = new Map((response.headers.get("Cache-Control") ?? "").toLowerCase()
        .split(",").map(directive => directive.trim().split("=")));
    const cacheable = !(directives.has("no-store") || directives.has("no-cache"));
    const maxAge = directives.has("max-age")
        ? Math.max(0, Number(directives.get("max-age")?.replaceAll('"', "")) - Number(response.headers.get("Age")))
        : null;
    return [new Blob(chunks, { type: response.headers.get("Content-Type") ?? "" }), cacheable, maxAge];
}
