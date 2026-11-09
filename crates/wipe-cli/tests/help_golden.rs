//! Pins what `wipestation --help` and every subcommand's `--help` print
//! (drift-tests story 2).
//!
//! The built binary is run with an emptied environment; commands are
//! discovered from its own `Commands:` sections, recursively, so a new or
//! removed subcommand is noticed without a hand-kept list. Each command path
//! has a golden `docs/generated/cli/wipestation[-<sub>...].txt` holding
//! exactly its stdout. Missing, stale and differing goldens all fail;
//! regenerate with `UPDATE_GOLDENS=1 cargo test -p wipe-cli --test help_golden`,
//! which also deletes goldens no command produces.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

const REGEN: &str = "UPDATE_GOLDENS=1 cargo test -p wipe-cli --test help_golden";
const GOLDEN_DIR: &str = "docs/generated/cli";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs `wipestation <path...> --help` with only `env` set and returns stdout.
fn help(path: &[String], env: &[(&str, &str)]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_wipestation"))
        .args(path)
        .arg("--help")
        .env_clear()
        .envs(env.iter().copied())
        .output()
        .unwrap_or_else(|e| panic!("running wipestation {path:?} --help: {e}"));
    assert!(
        out.status.success(),
        "wipestation {path:?} --help exited {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("help output is UTF-8")
}

/// Subcommand names listed in `help`'s `Commands:` section, minus clap's
/// built-in `help`.
fn commands(help: &str) -> Vec<String> {
    let mut lines = help.lines().skip_while(|l| l.trim_end() != "Commands:");
    if lines.next().is_none() {
        return Vec::new();
    }
    lines
        .take_while(|l| !l.trim().is_empty())
        // Entries are indented two spaces; anything deeper is a continuation.
        .filter(|l| l.starts_with("  ") && !l[2..].starts_with(char::is_whitespace))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_owned)
        .collect()
}

/// Golden file name for a command path: `wipestation[-<sub>...].txt`.
fn file_name(path: &[String]) -> String {
    let mut name = String::from("wipestation");
    for part in path {
        name.push('-');
        name.push_str(part);
    }
    name.push_str(".txt");
    name
}

/// Walks every command path from the top level, using `help_of` to get each
/// one's help text, and maps golden file name to that text.
fn collect(help_of: &dyn Fn(&[String]) -> String) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut queue: Vec<Vec<String>> = vec![Vec::new()];
    while let Some(path) = queue.pop() {
        let text = help_of(&path);
        for sub in commands(&text) {
            let mut child = path.clone();
            child.push(sub);
            queue.push(child);
        }
        out.insert(file_name(&path), text);
    }
    out
}

/// `*.txt` file names directly in `dir`; empty when it does not exist.
fn existing_goldens(dir: &Path) -> Result<Vec<String>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("reading {dir:?}: {e}")),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {dir:?}: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".txt") && entry.path().is_file() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Where `expected` and `actual` first differ: 1-based line number, golden
/// line, produced line (`<end of file>` when one side is shorter).
fn first_difference(expected: &str, actual: &str) -> (usize, String, String) {
    let mut exp = expected.split('\n');
    let mut act = actual.split('\n');
    let mut n = 0;
    loop {
        n += 1;
        match (exp.next(), act.next()) {
            (Some(e), Some(a)) if e == a => continue,
            (None, None) => unreachable!("first_difference called on equal text"),
            (e, a) => {
                let show = |s: Option<&str>| s.map_or("<end of file>".into(), |s| format!("{s:?}"));
                return (n, show(e), show(a));
            }
        }
    }
}

/// With `update`, makes `dir` hold exactly `produced` (creating it, deleting
/// stale `*.txt`) and succeeds; otherwise succeeds only when it already does.
fn check_dir(
    dir: &Path,
    rel: &str,
    produced: &BTreeMap<String, String>,
    update: bool,
) -> Result<(), String> {
    let existing = existing_goldens(dir)?;
    let stale: Vec<&String> = existing
        .iter()
        .filter(|n| !produced.contains_key(*n))
        .collect();

    if update {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
        for name in stale {
            std::fs::remove_file(dir.join(name))
                .map_err(|e| format!("deleting {rel}/{name}: {e}"))?;
        }
        for (name, text) in produced {
            std::fs::write(dir.join(name), text)
                .map_err(|e| format!("writing {rel}/{name}: {e}"))?;
        }
        return Ok(());
    }

    let mut problems = Vec::new();
    for (name, actual) in produced {
        let expected = match std::fs::read_to_string(dir.join(name)) {
            Ok(text) => text.replace("\r\n", "\n"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                problems.push(format!("  {rel}/{name}: missing (a command has no golden)"));
                continue;
            }
            Err(e) => {
                problems.push(format!("  {rel}/{name}: unreadable ({e})"));
                continue;
            }
        };
        if expected != *actual {
            let (line, golden, now) = first_difference(&expected, actual);
            problems.push(format!(
                "  {rel}/{name}: differs at line {line}\n      golden:   {golden}\n      produced: {now}"
            ));
        }
    }
    for name in stale {
        problems.push(format!("  {rel}/{name}: stale (no command produces it)"));
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "CLI help goldens are out of date:\n{}\nIf the change is intended, regenerate with `{REGEN}` and commit {rel}/.",
        problems.join("\n")
    ))
}

#[test]
fn help_goldens_match() {
    let produced = collect(&|path| help(path, &[]));
    let update = std::env::var("UPDATE_GOLDENS").as_deref() == Ok("1");
    if let Err(why) = check_dir(&repo_root().join(GOLDEN_DIR), GOLDEN_DIR, &produced, update) {
        panic!("{why}");
    }
}

#[test]
fn caller_env_does_not_change_help() {
    const MARK: &str = "/help-golden-caller-env";
    let serve = vec!["serve".to_owned()];
    let clean = help(&serve, &[]);

    // Sanity: a variable that reaches the binary does show in its help, so
    // the comparison below is meaningful.
    let leaked = help(&serve, &[("WIPESTATION_STATIC_DIR", MARK)]);
    assert!(leaked.contains(MARK), "{leaked}");

    // Set in this test process (the caller): the child must not see it.
    // Every test here clears the child's environment, so setting it while
    // other tests run cannot change their output either.
    std::env::set_var("WIPESTATION_STATIC_DIR", MARK);
    assert_eq!(help(&serve, &[]), clean);
}

// ---- pure-function tests ----

const TOP: &str = "\
wipestation — toolkit

Usage: wipestation <COMMAND>

Commands:
  serve        Run the server
  license      Licensing tools
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
";

const LICENSE: &str = "\
Licensing tools

Usage: wipestation license <COMMAND>

Commands:
  new-root  Generate a root
  issue     Issue a licence
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
";

const LEAF: &str = "Leaf\n\nUsage: wipestation x\n\nOptions:\n  -h, --help  Print help\n";

fn fake_help(path: &[String]) -> String {
    match path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => TOP.into(),
        ["license"] => LICENSE.into(),
        _ => LEAF.replace('x', &path.join(" ")),
    }
}

fn scratch_dir() -> PathBuf {
    // The counter keeps parallel tests apart; the clock alone is too coarse.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("help-golden-{}-{nanos}-{n}", std::process::id()))
}

#[test]
fn parses_commands_and_skips_help() {
    assert_eq!(commands(TOP), vec!["serve", "license"]);
    assert_eq!(commands(LICENSE), vec!["new-root", "issue"]);
    assert!(commands(LEAF).is_empty());
    // A deeper-indented line is a continuation, not a command.
    let wrapped = "Commands:\n  serve  Run\n         more words\n  inspect  Print\n\n";
    assert_eq!(commands(wrapped), vec!["serve", "inspect"]);
}

#[test]
fn collects_nested_command_paths() {
    let produced = collect(&fake_help);
    let names: Vec<&str> = produced.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        vec![
            "wipestation-license-issue.txt",
            "wipestation-license-new-root.txt",
            "wipestation-license.txt",
            "wipestation-serve.txt",
            "wipestation.txt",
        ]
    );
    assert_eq!(
        produced["wipestation-license-issue.txt"],
        fake_help(&["license".into(), "issue".into()])
    );
}

#[test]
fn regenerates_into_missing_dir_then_passes_and_tolerates_crlf() {
    let root = scratch_dir();
    let dir = root.join("nested/cli");
    let produced = collect(&fake_help);

    let why = check_dir(&dir, "cli", &produced, false).unwrap_err();
    assert!(why.contains("cli/wipestation.txt: missing"), "{why}");
    assert!(why.contains(REGEN), "{why}");

    check_dir(&dir, "cli", &produced, true).unwrap();
    check_dir(&dir, "cli", &produced, false).unwrap();

    let top = dir.join("wipestation.txt");
    std::fs::write(&top, TOP.replace('\n', "\r\n")).unwrap();
    check_dir(&dir, "cli", &produced, false).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn changed_help_names_file_and_first_differing_line() {
    let root = scratch_dir();
    let produced = collect(&fake_help);
    check_dir(&root, "cli", &produced, true).unwrap();

    let mut changed = produced.clone();
    changed.insert(
        "wipestation-serve.txt".into(),
        LEAF.replace("Leaf", "Edited").replace('x', "serve"),
    );
    let why = check_dir(&root, "cli", &changed, false).unwrap_err();
    assert!(
        why.contains("cli/wipestation-serve.txt: differs at line 1"),
        "{why}"
    );
    assert!(why.contains("golden:   \"Leaf\""), "{why}");
    assert!(why.contains("produced: \"Edited\""), "{why}");
    assert!(why.contains(REGEN), "{why}");
    assert!(
        !why.contains("wipestation.txt:"),
        "only the changed file is named: {why}"
    );

    // For newline-terminated text the golden side shows its empty final
    // line; `<end of file>` appears only when one side has no more segments.
    let (line, golden, now) = first_difference("a\n", "a\nb\n");
    assert_eq!((line, golden.as_str(), now.as_str()), (2, "\"\"", "\"b\""));
    let (line, golden, _) = first_difference("a", "a\nb");
    assert_eq!((line, golden.as_str()), (2, "<end of file>"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn added_command_fails_as_missing_golden() {
    let root = scratch_dir();
    let produced = collect(&fake_help);
    check_dir(&root, "cli", &produced, true).unwrap();

    let mut added = produced.clone();
    added.insert("wipestation-inspect.txt".into(), LEAF.into());
    let why = check_dir(&root, "cli", &added, false).unwrap_err();
    assert!(
        why.contains("cli/wipestation-inspect.txt: missing"),
        "{why}"
    );
    assert!(why.contains(REGEN), "{why}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn removed_command_fails_as_stale_and_update_deletes_it() {
    let root = scratch_dir();
    let produced = collect(&fake_help);
    check_dir(&root, "cli", &produced, true).unwrap();
    // A non-golden file is left alone.
    std::fs::write(root.join("README"), "keep").unwrap();

    let mut removed = produced.clone();
    removed.remove("wipestation-license-issue.txt");
    let why = check_dir(&root, "cli", &removed, false).unwrap_err();
    assert!(
        why.contains("cli/wipestation-license-issue.txt: stale"),
        "{why}"
    );
    assert!(why.contains(REGEN), "{why}");

    check_dir(&root, "cli", &removed, true).unwrap();
    assert!(!root.join("wipestation-license-issue.txt").exists());
    assert!(root.join("README").exists());
    check_dir(&root, "cli", &removed, false).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}
