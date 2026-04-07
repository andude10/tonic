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

/** Resolve a single-cell reference to its current value string. */
async function resolveRef(arg: ExternArg): Promise<string> {
    const v = arg.value as { sheet_id: number; row: number; col: number };
    return invoke<string>("resolve_reference", {
        sheetId: v.sheet_id,
        row: v.row,
        col: v.col,
    });
}

/** Convert a non-reference arg to its string value. */
function argToString(arg: ExternArg): string {
    switch (arg.type) {
        case "number":
        case "text":
            return arg.value as string;
        case "boolean":
            return String(arg.value);
        default:
            return String(arg.value);
    }
}

/** Resolve all args: fetch cell values for references, pass others through. */
async function resolveArgs(args: ExternArg[]): Promise<string[]> {
    return Promise.all(
        args.map((arg) => {
            if (arg.type === "single_ref") return resolveRef(arg);
            if (arg.type === "range_ref") return Promise.resolve("[range]"); // TODO: resolve range
            return Promise.resolve(argToString(arg));
        }),
    );
}

/**
 * Start listening for external function calls from the Rust eval engine.
 * For each call, resolves reference arguments, logs success, and sends the
 * resolved args back as the response.
 */
export function initExtensionDispatcher(): void {
    listen<ExtFnCall>("ext-fn-call", async (event) => {
        const { callId, funcName, args } = event.payload;

        try {
            const resolved = await resolveArgs(args);
            console.log(
                `[ext] ${funcName}(${resolved.join(", ")}) — resolved successfully`,
            );
            await emit(`ext-fn-response-${callId}`, resolved.join(", "));
        } catch (err) {
            console.error(`[ext] ${funcName} call ${callId} failed:`, err);
            await emit(`ext-fn-response-${callId}`, `ERROR: ${String(err)}`);
        }
    });
}
