// Capture the committed box before retirement removes it from browser layout.
export function octara_exit_style(element, visibleOnly) {
    if (!element.isConnected || element.ownerDocument.hidden || typeof element.animate !== "function") return null;
    const view = element.ownerDocument.defaultView;
    const rect = element.getBoundingClientRect();
    const elementStyle = view.getComputedStyle(element);
    if (!rect.width || !rect.height || elementStyle.visibility !== "visible") return null;
    let left = 0, top = 0, right = view.innerWidth, bottom = view.innerHeight;
    let transformed = elementStyle.transform !== "none";
    for (let parent = element.parentElement; parent; parent = parent.parentElement) {
        const style = view.getComputedStyle(parent);
        if (Number(style.opacity) <= 0) return null;
        transformed ||= style.transform !== "none";
        if (style.overflowX === "visible" && style.overflowY === "visible") continue;
        const box = parent.getBoundingClientRect();
        if (style.overflowX !== "visible") {
            const start = box.x + parent.clientLeft;
            left = Math.max(left, start);
            right = Math.min(right, start + parent.clientWidth);
        }
        if (style.overflowY !== "visible") {
            const start = box.y + parent.clientTop;
            top = Math.max(top, start);
            bottom = Math.min(bottom, start + parent.clientHeight);
        }
    }
    if (visibleOnly && (Math.max(top, rect.top) >= Math.min(bottom, rect.bottom)
        || Math.max(left, rect.left) >= Math.min(right, rect.right))) return null;
    const frozen = `${element.style.cssText};color-scheme:${elementStyle.colorScheme};inset:auto;margin:0;box-sizing:border-box;
        min-width:0;min-height:0;max-width:none;max-height:none;pointer-events:none;transition:none;`;
    if (transformed) {
        // Keep the local box and let the browser retain the transform and ancestor clipping.
        const position = elementStyle.position === "fixed" ? "fixed" : "absolute";
        return `${frozen}position:${position};left:${element.offsetLeft}px;top:${element.offsetTop}px;
            width:${elementStyle.width};height:${elementStyle.height};`;
    }
    const clip = [top - rect.top, rect.right - right, rect.bottom - bottom, left - rect.left]
        .map(value => `${value}px`).join(" ");
    return `${frozen}position:fixed;left:${rect.x}px;top:${rect.y}px;width:${rect.width}px;height:${rect.height}px;clip-path:inset(${clip});`;
}

export function octara_exit_opacity(element) {
    return Number(element.ownerDocument.defaultView.getComputedStyle(element).opacity);
}

export function octara_exit_reduced_motion(element) {
    return element.ownerDocument.defaultView.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function octara_exit_start(element, opacity, duration) {
    return element.animate({ opacity: Array.from(opacity) }, { duration, fill: "forwards" });
}

export function octara_exit_remaining(animation, element) {
    if (!animation || !element.isConnected || element.ownerDocument.hidden || octara_exit_reduced_motion(element)
        || animation.playState === "finished" || animation.playState === "idle" || !element.getClientRects().length) return 0;
    return Math.max(1, animation.effect.getComputedTiming().endTime - (animation.currentTime ?? 0));
}

export function octara_exit_cancel(animation) {
    animation?.cancel();
}
