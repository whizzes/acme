//! Workspace task runner (spec §22.1's restructure, pulled forward for
//! this one binary/lint only — see specs/003-First-Vertical-Slice.md's own
//! Caveats: no `progenitor`/codegen/`acme-client` scaffold here, just the
//! `spec-lint` check §22.14 schedules into M2).

mod export_openapi;
mod spec_lint;

const KNOWN_SUBCOMMANDS: &str = "spec-lint, export-openapi";

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("spec-lint") => {
            if !spec_lint::run() {
                std::process::exit(1);
            }
        }
        Some("export-openapi") => {
            let mut pay_out = "assets/openapi.json".to_string();
            let mut ship_out = "assets/openapi-ship.json".to_string();
            let mut rest = args;
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--out" => pay_out = rest.next().expect("--out expects a path"),
                    "--out-ship" => ship_out = rest.next().expect("--out-ship expects a path"),
                    other => {
                        eprintln!("xtask export-openapi: unknown flag `{other}`");
                        std::process::exit(2);
                    }
                }
            }
            export_openapi::run(&pay_out, &ship_out);
        }
        Some(other) => {
            eprintln!("xtask: unknown subcommand `{other}` (known: {KNOWN_SUBCOMMANDS})");
            std::process::exit(2);
        }
        None => {
            eprintln!("xtask: expected a subcommand (known: {KNOWN_SUBCOMMANDS})");
            std::process::exit(2);
        }
    }
}
