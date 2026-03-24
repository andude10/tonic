let _showNotice: ((msg: { text: string; type?: string }) => void) | null = null;

export function initNotice(fn: typeof _showNotice) {
    _showNotice = fn;
}

export function showError(text: string) {
    if (_showNotice) _showNotice({ text, type: "danger" });
    else console.error(text);
}
