export function octara_measure_batch(surface, elements, out) {
    const origin = surface.getBoundingClientRect();
    for (let i = 0, o = 0; i < elements.length; ++i, o += 18) {
        const el = elements[i];
        const rect = el.getBoundingClientRect();
        const box = rect.width || rect.height || rect.x || rect.y || el.getClientRects().length;
        out[o] = box ? rect.x - origin.x : 0;
        out[o + 1] = box ? rect.y - origin.y : 0;
        out[o + 2] = rect.width;
        out[o + 3] = rect.height;
        out[o + 4] = el.clientWidth;
        out[o + 5] = el.clientHeight;
        out[o + 6] = el.scrollWidth;
        out[o + 7] = el.scrollHeight;
        out[o + 8] = el.scrollLeft;
        out[o + 9] = el.scrollTop;
        const css = el.ownerDocument.defaultView.getComputedStyle(el);
        out[o + 10] = parseFloat(css.paddingTop) || 0;
        out[o + 11] = parseFloat(css.paddingRight) || 0;
        out[o + 12] = parseFloat(css.paddingBottom) || 0;
        out[o + 13] = parseFloat(css.paddingLeft) || 0;
        out[o + 14] = parseFloat(css.borderTopWidth) || 0;
        out[o + 15] = parseFloat(css.borderRightWidth) || 0;
        out[o + 16] = parseFloat(css.borderBottomWidth) || 0;
        out[o + 17] = parseFloat(css.borderLeftWidth) || 0;
    }
}

export function octara_text_anchor(surface, element, caret, points) {
    const doc = element.ownerDocument;
    let rect;
    if (points.length === 2) {
        rect = fieldRange(doc, element, points[0], points[1]);
    } else {
        const range = doc.createRange();
        if (points.length === 4) {
            range.setStart(points[0], points[1]);
            range.setEnd(points[2], points[3]);
        } else {
            const selection = doc.getSelection();
            if (!selection?.rangeCount || selection.isCollapsed || caret) return null;
            const selected = selection.getRangeAt(0);
            if (!selected.intersectsNode(element)) return null;
            range.selectNodeContents(element);
            if (range.compareBoundaryPoints(Range.START_TO_START, selected) < 0) {
                range.setStart(selected.startContainer, selected.startOffset);
            }
            if (range.compareBoundaryPoints(Range.END_TO_END, selected) > 0) {
                range.setEnd(selected.endContainer, selected.endOffset);
            }
            if (range.collapsed) return null;
        }
        rect = range.getBoundingClientRect();
        if (rect.height <= 0) {
            const start = adjacentCaret(doc, element, range.startContainer, range.startOffset);
            if (caret) {
                rect = start ?? fieldRange(doc, element, 0, 0);
            } else if (start && rect.width > 0) {
                rect = { x: rect.x, y: start.y, width: rect.width, height: start.height };
            }
        }
    }
    if (!rect || rect.height <= 0) return null;
    const origin = surface.getBoundingClientRect();
    return [rect.x - origin.x, rect.y - origin.y, caret ? Math.max(1, rect.width) : rect.width, rect.height];
}

function adjacentCaret(doc, element, container, offset) {
    if (container.nodeType === Node.TEXT_NODE && container.length > 0) {
        const range = doc.createRange();
        range.setStart(container, offset);
        range.collapse(true);
        const rect = range.getBoundingClientRect();
        return rect.height > 0 ? rect : null;
    }
    if (container.nodeType !== Node.ELEMENT_NODE) return null;
    const css = doc.defaultView.getComputedStyle(element);
    for (const [child, leading] of [[container.childNodes[offset], true], [container.childNodes[offset - 1], false]]) {
        if (child?.nodeType === Node.TEXT_NODE && child.length > 0) {
            const rect = adjacentCaret(doc, element, child, leading ? 0 : child.length);
            if (rect) return rect;
        }
        const rect = child?.getBoundingClientRect?.();
        if (rect && (rect.height > 0 || rect.width > 0)) {
            const x = leading !== (css.direction === "rtl") ? rect.left : rect.right;
            const height = rect.height || parseFloat(css.lineHeight) || parseFloat(css.fontSize);
            return { x, y: rect.height ? rect.y : rect.y - height, width: 1, height };
        }
    }
    return null;
}

function fieldRange(doc, field, start, end) {
    const css = doc.defaultView.getComputedStyle(field);
    const fieldRect = field.getBoundingClientRect();
    const mirror = doc.createElement("div");
    for (const name of [
        "font", "font-kerning", "font-variant", "line-height", "letter-spacing", "word-spacing",
        "text-align", "text-indent", "text-transform", "direction", "tab-size",
        "padding", "overflow-wrap", "word-break",
    ]) {
        mirror.style.setProperty(name, css.getPropertyValue(name));
    }
    const single = field.tagName === "INPUT";
    mirror.style.position = "fixed";
    mirror.style.left = `${fieldRect.left + field.clientLeft - field.scrollLeft}px`;
    const lineHeight = parseFloat(css.lineHeight) || parseFloat(css.fontSize);
    const padding = parseFloat(css.paddingTop) + parseFloat(css.paddingBottom);
    const offset = single ? Math.max(0, (field.clientHeight - lineHeight - padding) / 2) : 0;
    mirror.style.top = `${fieldRect.top + field.clientTop - field.scrollTop + offset}px`;
    mirror.style.width = `${field.clientWidth}px`;
    mirror.style.boxSizing = "border-box";
    mirror.style.visibility = "hidden";
    mirror.style.pointerEvents = "none";
    mirror.style.whiteSpace = single ? "pre" : "pre-wrap";
    const value = field.type === "password" ? "•".repeat(field.value.length) : (field.value ?? "");
    const text = doc.createTextNode(value + "\u200b");
    mirror.append(text);
    doc.body.append(mirror);
    try {
        const range = doc.createRange();
        range.setStart(text, start);
        range.setEnd(text, end);
        const rect = range.getBoundingClientRect();
        const x = Math.max(rect.left, fieldRect.left);
        const y = Math.max(rect.top, fieldRect.top);
        return {
            x, y,
            width: Math.max(0, Math.min(rect.right, fieldRect.right) - x),
            height: Math.max(0, Math.min(rect.bottom, fieldRect.bottom) - y),
        };
    } finally {
        mirror.remove();
    }
}
