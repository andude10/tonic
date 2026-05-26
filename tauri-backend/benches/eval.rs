use std::{fmt, path::Path};

use divan::Bencher;
use tonic_lib::{engine::Engine, storage::types::AbsoluteCellId};

#[derive(Clone, Copy)]
struct BenchFile {
    label: &'static str,
    path: &'static str,
    rows: u32,
}

const SUM_10M_FILE: BenchFile = BenchFile {
    label: "10M cells / 1M formulas",
    path: "tauri-backend/test-files/unversioned-10M-cells-1M-formulas-sum-of-10M.tcs",
    rows: 1_000_000,
};

impl fmt::Display for BenchFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label)
    }
}

fn main() {
    divan::main();
}

#[allow(non_snake_case)]
#[divan::bench(args = [SUM_10M_FILE], sample_count = 5, sample_size = 1)]
fn delete_H_column(bencher: Bencher, file: BenchFile) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tauri-backend should be inside the project")
        .join(file.path);

    bencher
        .with_inputs(move || {
            let mut engine = Engine::new();

            // open once per measured iteration, outside timed section.
            engine
                .open_spreadsheet(path.to_str().expect("benchmark path should be UTF-8"))
                .expect("benchmark input file should open");
            engine
        })
        .bench_local_values(move |mut engine| {
            let guard = engine.start_batch();

            // delete H-column
            for row in 0..file.rows {
                engine.delete(
                    &guard,
                    AbsoluteCellId {
                        sheet_id: 0,
                        row,
                        col: 7,
                    },
                );
            }

            engine.end_batch(guard);

            // return engine so dropping the opened file is also outside timing.
            divan::black_box(engine)
        });
}
