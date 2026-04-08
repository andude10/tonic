// text.js — text processing functions for tonic
//
// attach via View > Extensions > Add file
//
// usage:
//   =slugify("Hello World!")         -> "hello-world"
//   =wordcount("one two three")      -> 3
//   =extract("price: $42.50", "\d+\.\d+")  -> "42.50"
//   =join(A1:A5, ", ")               -> "a, b, c, d, e"
//   =transpose(A1:C2)               used as a range result (3x2 -> 2x3)
//   =fuzzy("kitten", "sitting")      -> 3 (levenshtein distance)

tonic.registerFunction("slugify", {
    params: { text: "any" },
    returns: "text",
    fn: (text) =>
        String(text)
            .toLowerCase()
            .trim()
            .replace(/[^\w\s-]/g, "")
            .replace(/[\s_]+/g, "-")
            .replace(/^-+|-+$/g, ""),
});

tonic.registerFunction("wordcount", {
    params: { text: "any" },
    returns: "number",
    fn: (text) => {
        const s = String(text).trim();
        if (s === "") return 0;
        return s.split(/\s+/).length;
    },
});

tonic.registerFunction("extract", {
    params: { text: "any", pattern: "text" },
    returns: "text",
    fn: (text, pattern) => {
        try {
            const match = String(text).match(new RegExp(pattern));
            return match ? match[0] : "";
        } catch (e) {
            return new Error("bad regex: " + e.message);
        }
    },
});

// join all non-empty cells in a range with a separator
tonic.registerFunction("join", {
    params: { range: "any", sep: "text" },
    returns: "text",
    fn: (range, sep) => {
        const values = [];
        for (const row of range) {
            for (const cell of row) {
                if (cell !== "") values.push(cell);
            }
        }
        return values.join(sep);
    },
});

// levenshtein edit distance between two strings
tonic.registerFunction("fuzzy", {
    params: { a: "text", b: "text" },
    returns: "number",
    fn: (a, b) => {
        a = String(a);
        b = String(b);
        const m = a.length,
            n = b.length;
        const dp = Array.from({ length: m + 1 }, (_, i) => {
            const row = new Array(n + 1);
            row[0] = i;
            return row;
        });
        for (let j = 1; j <= n; j++) dp[0][j] = j;
        for (let i = 1; i <= m; i++) {
            for (let j = 1; j <= n; j++) {
                const cost = a[i - 1] === b[j - 1] ? 0 : 1;
                dp[i][j] = Math.min(
                    dp[i - 1][j] + 1,
                    dp[i][j - 1] + 1,
                    dp[i - 1][j - 1] + cost,
                );
            }
        }
        return dp[m][n];
    },
});
