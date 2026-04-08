import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";

interface ExternArg {
    type: "number" | "text" | "boolean" | "single_ref" | "range_ref";
    value: unknown;
}

interface ExtFnCall {
    callId: number;
    funcName: string;
    args: ExternArg[];
}

// rust sends {v, t} for success, {e} for error — must match ExtFnResponse in engine.rs
interface ExtFnResponse {
    v?: string;
    t?: string;
    e?: string;
}

// function name -> worker that owns it
const funcToWorker = new Map<string, Worker>();

// pending call callbacks keyed by callId
const pendingCalls = new Map<number, { resolve: (r: ExtFnResponse) => void }>();

// build a Reference enum matching the rust serde format
function toReference(arg: ExternArg): object {
    if (arg.type === "single_ref") {
        const v = arg.value as { sheet_id: number; row: number; col: number };
        return {
            Single: {
                sheet_id: v.sheet_id,
                row: { Absolute: v.row },
                col: { Absolute: v.col },
            },
        };
    }
    const v = arg.value as {
        sheet_id: number;
        start_row: number;
        start_col: number;
        end_row: number;
        end_col: number;
    };
    return {
        Range: {
            sheet_id: v.sheet_id,
            start_row: { Absolute: v.start_row },
            start_col: { Absolute: v.start_col },
            end_row: { Absolute: v.end_row },
            end_col: { Absolute: v.end_col },
        },
    };
}

const decoder = new TextDecoder();

// invoke resolve_reference (returns raw bytes), decode to string
async function resolveRef(arg: ExternArg): Promise<string> {
    const buf = await invoke<ArrayBuffer>("resolve_reference", {
        reference: toReference(arg),
    });
    return decoder.decode(buf);
}

// resolve all args to JS values
// references are resolved via resolve_reference (reads sheets directly, no lock conflict with eval)
async function resolveArgs(args: ExternArg[]): Promise<unknown[]> {
    return Promise.all(
        args.map(async (arg) => {
            if (arg.type === "number") return Number(arg.value);
            if (arg.type === "boolean") return arg.value;
            if (arg.type === "single_ref") {
                const str = await resolveRef(arg);
                const num = Number(str);
                return isNaN(num) ? str : num;
            }
            if (arg.type === "range_ref") {
                const json = await resolveRef(arg);
                try {
                    return JSON.parse(json);
                } catch {
                    return json;
                }
            }
            return String(arg.value);
        }),
    );
}

function handleWorkerMessage(worker: Worker, e: MessageEvent) {
    const msg = e.data;

    if (msg.type === "registered") {
        funcToWorker.set(msg.name, worker);
        invoke("register_function", {
            name: msg.name,
            args: msg.paramTypes,
            fileName: (worker as any).__fileName ?? "",
        }).catch((err) =>
            console.error("[ext] register_function failed:", err),
        );
        return;
    }

    if (msg.type === "call-result" || msg.type === "call-error") {
        const pending = pendingCalls.get(msg.callId);
        if (!pending) return;
        pendingCalls.delete(msg.callId);
        if (msg.type === "call-error") {
            pending.resolve({ e: msg.error });
        } else {
            pending.resolve({ v: msg.value, t: msg.returnType ?? "text" });
        }
        return;
    }

    if (msg.type === "load-error") {
        console.error(`[ext] failed to load ${msg.fileName}:`, msg.error);
    }
}

export function loadExtension(fileName: string, code: string): Worker {
    const worker = new Worker(new URL("./worker-sandbox.ts", import.meta.url), {
        type: "module",
    });
    (worker as any).__fileName = fileName;
    worker.onmessage = (e) => handleWorkerMessage(worker, e);
    worker.postMessage({ type: "load", fileName, code });
    return worker;
}

export function unloadExtension(fileName: string): void {
    for (const [name, worker] of funcToWorker.entries()) {
        if ((worker as any).__fileName === fileName) {
            funcToWorker.delete(name);
        }
    }
}

export function initExtensionDispatcher(): void {
    listen<ExtFnCall>("ext-fn-call", async (event) => {
        const { callId, funcName, args } = event.payload;

        try {
            const resolved = await resolveArgs(args);
            const worker = funcToWorker.get(funcName);

            if (!worker) {
                await emit(`ext-fn-response-${callId}`, {
                    e: `function '${funcName}' not found`,
                });
                return;
            }

            // send to worker, wait for response
            const resp = await new Promise<ExtFnResponse>((resolve) => {
                pendingCalls.set(callId, { resolve });
                worker.postMessage({
                    type: "call",
                    callId,
                    funcName,
                    args: resolved,
                });
            });

            await emit(`ext-fn-response-${callId}`, resp);
        } catch (err) {
            await emit(`ext-fn-response-${callId}`, { e: String(err) });
        }
    });
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
