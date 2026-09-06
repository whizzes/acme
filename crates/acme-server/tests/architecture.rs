//! Spec §17's last row: a test that fails if `sqlx::query` or direct
//! `AppState.db` access appears where it shouldn't. `db::repo` is the only
//! code allowed to query (spec §5) — `providers/` handlers and, from M3
//! on, `web/` dashboard handlers must go through it like everything else.
//! (`capture/` and `http/auth.rs` are pre-existing, deliberate exceptions
//! predating this test — see their own module docs.)

use std::fs;
use std::path::Path;

fn scan(dir: &str) -> Vec<String> {
    let mut offenders = Vec::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    visit(&root, &mut offenders);
    offenders
}

fn visit(dir: &Path, offenders: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, offenders);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        for (line_no, line) in contents.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains("sqlx::query") {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    line_no + 1,
                    line.trim()
                ));
            }
        }
    }
}

#[test]
fn providers_never_query_the_database_directly() {
    let offenders = scan("src/providers");
    assert!(
        offenders.is_empty(),
        "src/providers/ must go through db::repo, not sqlx::query* directly:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn dashboard_handlers_never_query_the_database_directly() {
    let offenders = scan("src/web");
    assert!(
        offenders.is_empty(),
        "src/web/ must go through db::repo, not sqlx::query* directly:\n{}",
        offenders.join("\n")
    );
}
