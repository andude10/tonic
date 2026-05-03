// stats.js - statistical functions for tonic
//
// attach via View > Extensions > Add file
//
// usage:
//   =median(A1:A100)      median of a range
//   =countif(A1:A20, 42)  count cells that equal a value

// flatten a 2D string array from a range into numbers, skipping blanks
function numbersFromRange(range) {
    const nums = [];
    for (const row of range) {
        for (const cell of row) {
            if (cell === "") continue;
            const n = Number(cell);
            if (!isNaN(n)) nums.push(n);
        }
    }
    return nums;
}

tonic.registerFunction("median", {
    params: { range: "any" },
    returns: "number",
    fn(range) {
        const nums = numbersFromRange(range).sort((a, b) => a - b);
        if (nums.length === 0) return 0;
        const mid = Math.floor(nums.length / 2);
        return nums.length % 2 !== 0
            ? nums[mid]
            : (nums[mid - 1] + nums[mid]) / 2;
    },
});

tonic.registerFunction("countif", {
    params: { range: "any", target: "any" },
    returns: "number",
    fn(range, target) {
        const t = String(target);
        let count = 0;
        for (const row of range) {
            for (const cell of row) {
                if (cell === t) count++;
            }
        }
        return count;
    },
});
