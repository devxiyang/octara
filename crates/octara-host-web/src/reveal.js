// One custom highlight per sampled color. DOM text and line boxes stay intact.
const surfaces = new WeakMap();
let serial = 0;

export function octara_clear_reveals(surface) {
    const previous = surfaces.get(surface);
    if (!previous) return;
    for (const name of previous.names) previous.registry.delete(name);
    previous.style.remove();
    surfaces.delete(surface);
}

export function octara_sample_reveals(surface, samples) {
    octara_clear_reveals(surface);
    const doc = surface.ownerDocument;
    const browser = doc.defaultView;
    const registry = browser.CSS.highlights;
    if (!registry || !browser.Highlight || !samples.length) return;
    const colors = new Map();
    for (const [element, opacity, tint] of samples) {
        let offset = 0;
        function walk(node) {
            if (node.nodeType === 3) {
                const length = node.length;
                let start = 0;
                let base;
                while (start < length) {
                    const alpha = opacity[offset + start] ?? 255;
                    let end = start + 1;
                    while (end < length && (opacity[offset + end] ?? 255) === alpha) end++;
                    if (alpha !== 255) {
                        let color = "transparent";
                        if (alpha !== 0) {
                            base ??= browser.getComputedStyle(node.parentElement).color;
                            const amount = 100 * alpha / 255;
                            const mixed = tint ? `color-mix(in srgb, ${base} ${amount}%, ${tint})` : base;
                            color = `color-mix(in srgb, ${mixed} ${amount}%, transparent)`;
                        }
                        const range = doc.createRange();
                        range.setStart(node, start);
                        range.setEnd(node, end);
                        if (!colors.has(color)) colors.set(color, new browser.Highlight());
                        colors.get(color).add(range);
                    }
                    start = end;
                }
                offset += length;
            } else if (node.nodeType === 1) {
                const source = node.getAttribute("data-octara-inline-source");
                if (source !== null) offset += source.length;
                else for (const child of node.childNodes) walk(child);
            }
        }
        walk(element);
    }
    if (!colors.size) return;
    const style = doc.createElement("style");
    const names = [];
    const rules = [];
    for (const [color, highlight] of colors) {
        const name = `octara-reveal-${serial++}`;
        registry.set(name, highlight);
        names.push(name);
        rules.push(`::highlight(${name}) { color: ${color}; }`);
    }
    style.textContent = rules.join("\n");
    doc.head.append(style);
    surfaces.set(surface, { registry, names, style });
}
