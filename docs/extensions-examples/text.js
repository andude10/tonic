// text.js - text processing functions for tonic
//
// attach via View > Extensions > Add file
//
// usage:
//   =wordcount("one two three")      -> 3
//   =extract("price: $42.50", "\d+\.\d+")  -> "42.50"
//   =join(A1:A5, ", ")               -> "a, b, c, d, e"

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

