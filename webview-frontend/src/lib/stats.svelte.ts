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
