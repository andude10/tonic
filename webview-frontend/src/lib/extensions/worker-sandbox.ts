// web worker sandbox for running extension scripts
//
// extension API:
//   tonic.registerFunction("double", (x) => x * 2)
//   tonic.registerFunction("add", ["number", "number"], (a, b) => a + b)
//   tonic.registerFunction("double", { params: { x: "number" }, returns: "number", fn: (x) => x * 2 })

interface FuncEntry {
    paramTypes: string[];
    returns: string;
    fn: Function;
}

interface FuncDescriptor {
    params: Record<string, string>;
    returns?: string;
    fn: Function;
}

const registry = new Map<string, FuncEntry>();

function coerce(value: unknown, type: string): unknown {
    if (type === "any") return value;
    if (type === "number") {
        if (typeof value === "number") return value;
        const n = Number(value);
        if (!isNaN(n)) return n;
        throw new TypeError(`expected number, got ${typeof value}`);
    }
    if (type === "text") return String(value);
    if (type === "boolean") return Boolean(value);
    return value;
}

function detectReturnType(value: unknown): string {
    if (typeof value === "number" && isFinite(value)) return "number";
    if (typeof value === "boolean") return "boolean";
    return "text";
}

const tonic = {
    registerFunction(
        name: string,
        second: string[] | Function | FuncDescriptor,
        third?: Function,
    ) {
        let paramTypes: string[];
        let returns: string;
        let fn: Function;

        if (typeof second === "function") {
            // shorthand: tonic.registerFunction("name", fn)
            fn = second;
            paramTypes = Array(fn.length).fill("any");
            returns = "any";
        } else if (Array.isArray(second)) {
            // array form: tonic.registerFunction("name", ["number"], fn)
            paramTypes = second;
            fn = third!;
            returns = "any";
        } else {
            // object form: tonic.registerFunction("name", { params, returns, fn })
            const desc = second;
            paramTypes = Object.values(desc.params);
            returns = desc.returns ?? "any";
            fn = desc.fn;
        }

        registry.set(name, { paramTypes, returns, fn });
        self.postMessage({ type: "registered", name, paramTypes, returns });
    },
};

(self as any).tonic = tonic;

self.onmessage = (e: MessageEvent) => {
    const msg = e.data;

    if (msg.type === "load") {
        try {
            const indirectEval = eval;
            indirectEval(msg.code);
            self.postMessage({ type: "loaded", fileName: msg.fileName });
        } catch (err) {
            self.postMessage({
                type: "load-error",
                fileName: msg.fileName,
                error: String(err),
            });
        }
        return;
    }

    if (msg.type === "call") {
        const { callId, funcName, args } = msg;
        const reg = registry.get(funcName);
        if (!reg) {
            self.postMessage({
                type: "call-error",
                callId,
                error: `function '${funcName}' not found`,
            });
            return;
        }
        try {
            // coerce args to declared types
            const coerced = (args as unknown[]).map((a, i) =>
                coerce(a, reg.paramTypes[i] ?? "any"),
            );
            const result = reg.fn(...coerced);

            // if extension explicitly throws, it becomes a cell error
            if (result instanceof Error) {
                self.postMessage({
                    type: "call-error",
                    callId,
                    error: result.message,
                });
                return;
            }

            const returnType =
                reg.returns === "any" ? detectReturnType(result) : reg.returns;
            self.postMessage({
                type: "call-result",
                callId,
                value: String(result),
                returnType,
            });
        } catch (err) {
            self.postMessage({
                type: "call-error",
                callId,
                error: String(err),
            });
        }
    }
};
