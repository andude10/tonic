// math.js — example tonic extension
//
// attach this file via View > Extensions > Add file
// then use in formulas: =double(A1), =clamp(B2, 0, 100), =safediv(10, 0)

// object form: declare param types and return type
// the worker validates at call time and tags the return value correctly
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
tonic.registerFunction("safediv", {
    params: { a: "number", b: "number" },
    returns: "number",
    fn: (a, b) => (b === 0 ? new Error("division by zero") : a / b),
});
