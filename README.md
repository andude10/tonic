## Tonic

![Showcase](./docs\screenshots\simple-showcase.png)

Tonic is an experimental desktop spreadsheet for Windows, Linux, and macOS. It aims to implement new, fun features that usually do not exist in classic spreadsheets, for example:

- Any cell can be resized with double Shift
- Extrapolation of formulas is done using "Clone" (Ctrl+D)
- Filtering and sorting work per-cell, not per-row
- Formulas behave more like a real programming language
- Descriptive formula errors
- User-defined formulas in JS

The main goal of this project is experimentation, which is why support for common spreadsheet file formats (such as .ods and .xls) is not implemented. However, this might change in the future. The project is still a work in progress, and there is a lot to do before it is ready for daily use.

### Getting started

Download the executable from the [release page](https://github.com/andude10/tonic/releases)

or build from source:

1. Install Rust and pnpm
2. Install Tauri CLI (`cargo install tauri-cli`)
3. Fetch frontend dependencies (`cd webview-frontend && pnpm install`)
4. Run dev build (`cargo tauri dev`)

### Implementation

Tonic is a Tauri desktop program written in Rust. The frontend uses Svelte, and the grid is built on top of [SVAR](https://svar.dev/svelte/datagrid/). The backend implements the spreadsheet engine, including the [TACO dependency graph](https://arxiv.org/pdf/2302.05482), parallel formula re-calculation, etc.
