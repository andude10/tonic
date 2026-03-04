import { listen } from "@tauri-apps/api/event";

export const devPanel = $state({
    starts: {} as Record<string, number>,
    stats: {} as Record<string, number>,
    highestTime: {} as Record<string, number>,
    counts: {} as Record<string, number>,
    fps: 0,
});

export function startTimer(name: string, count?: boolean) {
    devPanel.starts[name] = performance.now();
    if (count) {
        devPanel.counts[name] = (devPanel.counts[name] ?? 0) + 1;
    }
}

export function endTimer(name: string) {
    // do nothing if startTimer wasn't called before
    const start = devPanel.starts[name];
    if (start === undefined) return;
    delete devPanel.starts[name];

    // otherwise, save new time
    const time = performance.now() - start;
    devPanel.stats[name] = time;

    if (!(devPanel.highestTime[name] >= time)) {
        devPanel.highestTime[name] = time;
    }
}

export function resetTimer(name: string) {
    devPanel.stats[name] = 0;
    devPanel.highestTime[name] = 0;
    delete devPanel.counts[name];
}

export function trackFps() {
    const times = new Float64Array(240);
    let head = 0;
    let tail = 0;

    function loop() {
        const now = performance.now();
        // drop entries older than 1s
        while (head !== tail && times[head] <= now - 1000) {
            head = (head + 1) % 240;
        }
        times[tail] = now;
        tail = (tail + 1) % 240;
        devPanel.fps = tail >= head ? tail - head : 240 - head + tail;
        requestAnimationFrame(loop);
    }
    requestAnimationFrame(loop);
}

listen<[string, number, boolean]>("backend-timing", (event) => {
    const [name, ms, count] = event.payload;
    devPanel.stats[name] = ms;
    if (count) {
        devPanel.counts[name] = (devPanel.counts[name] ?? 0) + 1;
    }
    if (!(devPanel.highestTime[name] >= ms)) {
        devPanel.highestTime[name] = ms;
    }
});
