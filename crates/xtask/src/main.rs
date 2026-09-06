//! Workspace task runner (spec §22.1's restructure, pulled forward for
//! this one binary/lint only — see specs/003-First-Vertical-Slice.md's own
//! Caveats: no `progenitor`/codegen/`acme-client` scaffold here, just the
//! `spec-lint` check §22.14 schedules into M2).

mod spec_lint;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("spec-lint") => {
            if !spec_lint::run() {
                std::process::exit(1);
            }
        }
        Some(other) => {
            eprintln!("xtask: unknown subcommand `{other}` (known: spec-lint)");
            std::process::exit(2);
        }
        None => {
            eprintln!("xtask: expected a subcommand (known: spec-lint)");
            std::process::exit(2);
        }
    }
}
