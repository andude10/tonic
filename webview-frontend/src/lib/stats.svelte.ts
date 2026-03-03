import { tick } from "svelte";
import { listen } from "@tauri-apps/api/event";

export const devPanel = $state({
    starts: {} as Record<string, number>,
    stats: {} as Record<string, number>,
    highestTime: {} as Record<string, number>,
    counts: {} as Record<string, number>,
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

export function timeRenders() {
    startTimer("render");
    tick().then(() => {
        setTimeout(() => {
            endTimer("render");
            setTimeout(timeRenders, 500);
        }, 1);
    });
}

listen<[string, number]>("backend-timing", (event) => {
    const [name, ms] = event.payload;
    devPanel.stats[name] = ms;
    devPanel.counts[name] = (devPanel.counts[name] ?? 0) + 1;
    if (!(devPanel.highestTime[name] >= ms)) {
        devPanel.highestTime[name] = ms;
    }
});
