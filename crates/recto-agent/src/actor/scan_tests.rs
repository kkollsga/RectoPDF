//! **No rule about who may do what is stated again outside [`super::Actor`]** (AGENTS.md, "One action, one implementation": each rule is stated once
//! on the actor). A comparison of levels, or with the person, in the production code of `recto-agent`, `recto-control` or `recto-app` is a rule said
//! again, which can drift from its first saying, as the print's "anyway" once did: each is a method of `Actor`'s, or is in [`ALLOWED`] with why.

use crate::policy_guards::production_lines;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The comparisons that stay where they are, each with why: by the file's path from `crates/`, the words of its line, and the reason.
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "recto-control/src/server.rs",
        "if level == self.last_level {",
        "the server noticing the user changed the level, which is the level itself, not a rule about who may do what",
    ),
    (
        "recto-control/src/log.rs",
        "found.level == level",
        "the activity log counting repeats of the same command at the same level as one line: a key, not a rule",
    ),
    (
        "recto-control/src/server.rs",
        "if level == Level::Off {",
        "access turned off: the server lets go of what it was serving",
    ),
    (
        "recto-control/src/server.rs",
        "if self.last_level == Level::Off {",
        "with access off the server says so instead of answering: the level itself, not a rule about a command",
    ),
];

/// The comparison operators, as rustfmt writes a binary one: with a space each side, so `Vec<Level>` and `->` aren't one.
const OPERATORS: [&str; 6] = [" <= ", " >= ", " == ", " != ", " < ", " > "];

/// The line with what's in its string literals and after a `//` taken out, roughly: enough that words about a level in a message aren't code.
fn code_of(line: &str) -> String {
    let mut code = String::new();
    let (mut in_string, mut escaped) = (false, false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            match (escaped, c) {
                (false, '\\') => escaped = true,
                (false, '"') => {
                    in_string = false;
                    code.push('"');
                }
                _ => escaped = false,
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                code.push('"');
            }
            '/' if chars.peek() == Some(&'/') => break,
            _ => code.push(c),
        }
    }
    code
}

/// The operand either side of each comparison in `code`: what's next to the operator up to a space, a bracket or a comma.
fn operands(code: &str) -> Vec<String> {
    let edge = |c: char| c.is_whitespace() || matches!(c, '{' | '}' | '[' | ']' | ',' | ';' | '&' | '|');
    let mut found = Vec::new();
    for operator in OPERATORS {
        for (at, _) in code.match_indices(operator) {
            let left: String = code[..at]
                .chars()
                .rev()
                .take_while(|c| !edge(*c))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let right: String = code[at + operator.len()..].chars().take_while(|c| !edge(*c)).collect();
            found.extend([left, right]);
        }
    }
    found
}

/// Whether the line is a `match` arm whose pattern names an `Actor` variant (it starts with one, or goes on with `|`), not a closure or call that builds one.
fn arm_of_actor(code: &str) -> bool {
    let pattern = code.split("=>").next().unwrap_or("");
    code.contains("=>")
        && (pattern.trim_start().starts_with("Actor::")
            || pattern.contains("| Actor::")
            || pattern.contains("recto_agent::Actor::") && pattern.trim_start().starts_with("recto_agent"))
}

/// Whether a line takes an `Actor` apart: `matches!`, `if let`, `let ... else` or a `match` arm naming one of its variants. Building one
/// (`Actor::Person` as an argument) isn't.
fn takes_actor_apart(code: &str) -> bool {
    code.contains("Actor::")
        && (code.contains("matches!(") || code.contains("if let ") || code.contains("while let ") || code.contains("let Actor::") || arm_of_actor(code))
}

/// Whether a line turns a level into a number: `Level::X as u8`, `level as usize`. `Level` has no `<` or `.cmp`, so the compiler holds those; an
/// `as` cast of a fieldless enum is the way left to order levels.
fn casts_level(code: &str) -> bool {
    code.contains(" as ")
        && code.to_lowercase().contains("level")
        && ["u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize"]
            .iter()
            .any(|ty| code.contains(&format!(" as {ty}")))
}

/// Whether a production line compares levels, or an actor with the person, or takes an actor apart, or casts a level to a number: a level or an
/// actor beside a comparison, a `matches!` on a level.
fn compares_who(line: &str) -> bool {
    let code = code_of(line);
    let about_who = |operand: &str| operand.to_lowercase().contains("level") || operand.contains("Actor::");
    operands(&code).iter().any(|operand| about_who(operand))
        || (code.contains("matches!(") && code.contains("Level::"))
        || takes_actor_apart(&code)
        || casts_level(&code)
}

/// The files under `src` that are tests: a module declared under `#[cfg(test)]` or `#[cfg(any(test, ...))]`, with everything under it, wherever
/// it's declared. What's left is production code, of which [`production_lines`] then leaves out the test items inside a file.
pub(crate) fn test_files(src: &Path, files: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut test_dirs = Vec::new();
    let mut tests = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(file).unwrap();
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        // Where this file's modules are: beside it for `lib.rs` and `mod.rs`, in a folder of its name otherwise.
        let stem = file.file_stem().unwrap().to_string_lossy();
        let folder = file.parent().unwrap();
        let folder = if file == &src.join("lib.rs") || stem == "mod" {
            folder.to_path_buf()
        } else {
            folder.join(&*stem)
        };
        for pair in lines.windows(2) {
            let test_only = pair[0].starts_with("#[cfg(test)]") || pair[0].starts_with("#[cfg(any(test");
            let name = pair[1]
                .trim_start_matches("pub(crate) ")
                .trim_start_matches("pub ")
                .strip_prefix("mod ")
                .and_then(|rest| rest.strip_suffix(';'));
            if let (true, Some(name)) = (test_only, name) {
                tests.insert(folder.join(format!("{name}.rs")));
                test_dirs.push(folder.join(name));
            }
        }
    }
    tests.extend(files.iter().filter(|file| test_dirs.iter().any(|dir| file.starts_with(dir))).cloned());
    tests
}

/// Every `.rs` file under `folder`.
pub(crate) fn sources(folder: &Path) -> Vec<PathBuf> {
    let (mut folders, mut files) = (vec![folder.to_path_buf()], Vec::new());
    while let Some(folder) = folders.pop() {
        for path in std::fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().path()) {
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files
}

/// Failure: a rule about who may do what (a level, the owner's edits, a rewrite at Full, what an agent sees) is stated again in a crate's production
/// code by comparing levels or an actor with the person, outside `actor.rs`, where it can drift from the rule as `Actor` says it. Each such line of
/// `recto-agent`, `recto-control` and `recto-app` is a method of `Actor`'s, or in [`ALLOWED`] with its reason; and each allowed line is still there.
#[test]
fn no_rule_about_who_is_stated_outside_actor() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let (mut found, mut read, mut allowed_seen) = (Vec::new(), 0, BTreeSet::new());
    for name in ["recto-agent", "recto-control", "recto-app"] {
        let src = crates.join(name).join("src");
        let files = sources(&src);
        let tests = test_files(&src, &files);
        for file in files
            .iter()
            .filter(|file| !tests.contains(*file) && !file.ends_with("recto-agent/src/actor.rs"))
        {
            read += 1;
            let text = std::fs::read_to_string(file).unwrap();
            let shown = file.strip_prefix(&crates).unwrap().to_string_lossy().into_owned();
            for (at, line) in production_lines(&text) {
                if !compares_who(line) {
                    continue;
                }
                match ALLOWED.iter().position(|(path, words, _)| shown == *path && line.contains(words)) {
                    Some(entry) => {
                        allowed_seen.insert(entry);
                    }
                    None => found.push(format!("{shown}:{}: {}", at + 1, line.trim())),
                }
            }
        }
    }
    assert!(read > 150, "the crates' sources were read: {read} files");
    assert!(
        found.is_empty(),
        "a rule about who may do what, stated outside `Actor` (crates/recto-agent/src/actor.rs): ask a method of `Actor`'s, or add one there; or, if \
         it isn't such a rule, put it in ALLOWED with why:\n{}",
        found.join("\n")
    );
    let stale: Vec<&str> = (0..ALLOWED.len()).filter(|at| !allowed_seen.contains(at)).map(|at| ALLOWED[at].1).collect();
    assert!(stale.is_empty(), "no longer there, so out of ALLOWED: {stale:?}");
}

/// Failure: the scan misses a form of comparison it's for, takes one that isn't for one (a level in a type, a row's level, words in a message), or
/// reads a test module's lines as production or stops reading production code after a test module.
#[test]
fn the_scan_sees_each_comparison_and_nothing_else() {
    for seen in [
        "if level >= Level::Full {",
        "let protected = actor.level() < Level::Full;",
        "if cx.level < Level::Full && x {",
        "if self.unreadable && actor != Actor::Person {",
        "if actor == Actor::Person || !asleep {",
        "matches!(actor, Actor::Agent(Level::Full))",
        "if have >= level {",
        "if matches!(actor, Actor::Agent(_)) {",
        "if let Actor::Agent(level) = actor {",
        "let Actor::Agent(level) = actor else { return };",
        "Actor::Person => true,",
        "asked.filter(|asked| matches!(asked.by, recto_agent::Actor::Agent(_)))",
        "let rank = level as u8;",
        "let rank = Level::Full as usize;",
    ] {
        assert!(compares_who(seen), "{seen}");
    }
    for unseen in [
        "access: Access::At(Level::Full),",
        "fn f(levels: Vec<Level>) -> Option<Level> {",
        "let message = \"a level >= Full\";",
        "let x = a >= b; // the level >= Full",
        "Self::Agent(level) => allowed(level, needed),",
        "let actor = Actor::Person;",
        "host.run(Actor::Agent(level), &command, &mut events)",
        "let size = bytes as u64; // a level of detail",
        "if actor.is_agent() {",
    ] {
        assert!(!compares_who(unseen), "{unseen}");
    }
    let source = "fn a() {\n    if level >= Level::Full {}\n}\n#[cfg(test)]\nmod tests {\n    fn b() { if level >= Level::Full {} }\n}\nfn c() {\n    if level < Level::Control {}\n}\n";
    let lines: Vec<usize> = production_lines(source)
        .into_iter()
        .filter(|(_, line)| compares_who(line))
        .map(|(at, _)| at + 1)
        .collect();
    assert_eq!(lines, [2, 9], "production code before and after the test module, and not the test module's");
}
