(() => {
    const handlers = new Map();
    const message = window.webkit.messageHandlers.octara;
    Object.defineProperty(window, "octara", {
        value: Object.freeze({
            async call(method, args = null) {
                const reply = await message.postMessage(JSON.stringify({ method, args }));
                return JSON.parse(reply);
            },
            postMessage(payload) {
                return message.postMessage(JSON.stringify({ payload }));
            },
            handle(method, handler) {
                if (typeof handler !== "function") throw new TypeError("Expected a function");
                handlers.set(method, handler);
            },
            async dispatch(method, args) {
                const handler = handlers.get(method);
                if (!handler) throw new Error("Unknown Octara method");
                return await handler(args);
            },
        }),
    });
})();
