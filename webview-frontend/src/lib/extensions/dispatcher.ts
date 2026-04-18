import { invoke } from "@tauri-apps/api/core";
import { showError } from "$lib/notice";

const textDecoder = new TextDecoder();
const textEncoder = new TextEncoder();
const EMPTY_BODY = new Uint8Array();

// --- binary tags (mirrors ipc_encoding.rs) ---

const TAG_EMPTY = 0;
const TAG_TEXT = 1;
const TAG_NUMBER = 2;
const TAG_BOOL = 3;
const TAG_ERROR = 4;
const TAG_RANGE = 5;

// --- binary decoding ---

function readU32(view: DataView, o: { v: number }): number {
    const n = view.getUint32(o.v, true);
    o.v += 4;
    return n;
}

function readString(
    view: DataView,
    bytes: Uint8Array,
    o: { v: number },
): string {
    const len = readU32(view, o);
    const s = textDecoder.decode(bytes.subarray(o.v, o.v + len));
    o.v += len;
    return s;
}

function decodeScalar(
    view: DataView,
    bytes: Uint8Array,
    o: { v: number },
): unknown {
    const tag = bytes[o.v++];
    const s = readString(view, bytes, o);
    if (tag === TAG_NUMBER) return s === "" ? 0 : Number(s);
    if (tag === TAG_BOOL) return s === "true";
    return s;
}

function decodeArg(
    view: DataView,
    bytes: Uint8Array,
    o: { v: number },
): unknown {
    const tag = bytes[o.v++];
    if (tag === TAG_RANGE) {
        const rows = readU32(view, o);
        const cols = readU32(view, o);
        const result: unknown[][] = [];
        for (let r = 0; r < rows; r++) {
            const row: unknown[] = [];
            for (let c = 0; c < cols; c++)
                row.push(decodeScalar(view, bytes, o));
            result.push(row);
        }
        return result;
    }
    const s = readString(view, bytes, o);
    if (tag === TAG_NUMBER) return s === "" ? 0 : Number(s);
    if (tag === TAG_BOOL) return s === "true";
    return s;
}

// --- binary encoding (responses: flat sequence of CellValues) ---

interface CallResult {
    value: string;
    tag: number;
}

function encodeResponses(results: CallResult[]): Uint8Array {
    const encoded = results.map((r) => textEncoder.encode(r.value));
    let size = 0;
    for (const e of encoded) size += 1 + 4 + e.byteLength;
    const buf = new Uint8Array(size);
    const view = new DataView(buf.buffer);
    let o = 0;
    for (let i = 0; i < results.length; i++) {
        buf[o++] = results[i].tag;
        view.setUint32(o, encoded[i].byteLength, true);
        o += 4;
        buf.set(encoded[i], o);
        o += encoded[i].byteLength;
    }
    return buf;
}

// --- function registry (runs on main thread, no workers) ---

interface FuncEntry {
    paramTypes: string[];
    returns: string;
    fn: Function;
}

const registry = new Map<string, FuncEntry>();

function castArg(value: unknown, type: string): unknown {
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

// exposed to extension code via globalThis.tonic
const tonic = {
    registerFunction(
        name: string,
        second:
            | string[]
            | Function
            | {
                  params: Record<string, string>;
                  returns?: string;
                  fn: Function;
              },
        third?: Function,
    ) {
        let paramTypes: string[];
        let returns: string;
        let fn: Function;

        if (typeof second === "function") {
            fn = second;
            paramTypes = Array(fn.length).fill("any");
            returns = "any";
        } else if (Array.isArray(second)) {
            paramTypes = second;
            fn = third!;
            returns = "any";
        } else {
            paramTypes = Object.values(second.params);
            returns = second.returns ?? "any";
            fn = second.fn;
        }

        registry.set(name, { paramTypes, returns, fn });
        invoke("register_function", {
            name,
            args: paramTypes,
            fileName: currentLoadingFile,
        }).catch((err) => showError(String(err)));
    },
};

let currentLoadingFile = "";

function runCall(funcName: string, args: unknown[]): CallResult {
    const reg = registry.get(funcName);
    if (!reg)
        return { value: `function '${funcName}' not found`, tag: TAG_ERROR };
    try {
        const coerced = args.map((a, i) =>
            castArg(a, reg.paramTypes[i] ?? "any"),
        );
        const result = reg.fn(...coerced);
        if (result instanceof Error)
            return { value: result.message, tag: TAG_ERROR };
        const tag =
            reg.returns === "any" ? detectReturnType(result) : reg.returns;
        return {
            value: String(result),
            tag:
                tag === "number"
                    ? TAG_NUMBER
                    : tag === "boolean"
                      ? TAG_BOOL
                      : TAG_TEXT,
        };
    } catch (err) {
        return { value: String(err), tag: TAG_ERROR };
    }
}

// --- extension loading ---

const loadedFiles = new Set<string>();

export function loadExtension(fileName: string, code: string) {
    currentLoadingFile = fileName;
    loadedFiles.add(fileName);
    try {
        const indirectEval = eval;
        indirectEval(code);
    } catch (err) {
        console.error(`[ext] failed to load ${fileName}:`, err);
    }
    currentLoadingFile = "";
}

export function unloadExtension(fileName: string): void {
    loadedFiles.delete(fileName);
}

// --- poll loop ---

export function initExtensionDispatcher(): void {
    (globalThis as any).tonic = tonic;

    let responseBody: Uint8Array = EMPTY_BODY;

    function pollLoop() {
        const body = responseBody;
        responseBody = EMPTY_BODY;
        invoke<ArrayBuffer>("ext_fn_poll", body).then(
            (buf) => {
                if (buf.byteLength === 0) {
                    setTimeout(pollLoop, 16);
                    return;
                }
                const bytes = new Uint8Array(buf);
                const view = new DataView(buf);
                const o = { v: 0 };
                const results: CallResult[] = [];
                while (o.v < bytes.byteLength) {
                    const funcName = readString(view, bytes, o);
                    const argCount = readU32(view, o);
                    const args: unknown[] = [];
                    for (let i = 0; i < argCount; i++)
                        args.push(decodeArg(view, bytes, o));
                    results.push(runCall(funcName, args));
                }
                responseBody = encodeResponses(results);
                pollLoop();
            },
            () => setTimeout(pollLoop, 16),
        );
    }

    pollLoop();
}

export async function loadAllExtensions(): Promise<void> {
    const names = await invoke<string[]>("list_scripts");
    for (const name of names) {
        const content = await invoke<string | null>("get_script_content", {
            name,
        });
        if (content) loadExtension(name, content);
    }
}
