// math.js - example tonic extension
//
// attach this file via View > Extensions > Add file
// then use in formulas: =double(A1), =clamp(B2, 0, 100), =sqrt(A1), =product(A1:B10)

tonic.registerFunction("double", {
    params: { x: "number" },
    returns: "number",
    fn: (x) => x * 2,
});

tonic.registerFunction("clamp", {
    params: { value: "number", min: "number", max: "number" },
    returns: "number",
    fn: (value, min, max) => Math.min(Math.max(value, min), max),
});

// shorthand: just name + function, types inferred at runtime
tonic.registerFunction("hypot", (a, b) => Math.sqrt(a * a + b * b));

// returning an Error makes it a cell error
tonic.registerFunction("sqrt", {
    params: { x: "number" },
    returns: "number",
    fn: (x) => (x < 0 ? new Error("sqrt of negative") : Math.sqrt(x)),
});

// range argument: receives a 2D array of cell values
tonic.registerFunction("product", {
    params: { range: "any" },
    returns: "number",
    fn: (range) => {
        let result = 1;
        for (const row of range)
            for (const cell of row)
                if (cell !== "") result *= Number(cell) || 0;
        return result;
    },
});
