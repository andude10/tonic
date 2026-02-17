import { tick } from "svelte";

export const devPanel = $state({
    starts: {} as Record<string, number>,
    stats: {} as Record<string, number>,
    highestTime: {} as Record<string, number>,
});

export function startTimer(name: string) {
    devPanel.starts[name] = performance.now();
}

export function endTimer(name: string) {
    const time = performance.now() - devPanel.starts[name];

    devPanel.stats[name] = time;

    if (!devPanel.highestTime[name] || devPanel.highestTime[name] < time) {
        devPanel.highestTime[name] = time;
    }
}

export function resetTimer(name: string) {
    devPanel.stats[name] = 0;
    devPanel.highestTime[name] = 0;
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
