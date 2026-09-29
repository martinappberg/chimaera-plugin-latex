//! A LaTeX log read into problems (`diagnostics/1`), the heuristics LaTeX
//! Workshop and texlab use, ported: errors in `-file-line-error` form
//! (`./main.tex:12: Undefined control sequence.`) or `! …` with the `l.<n>`
//! context line, the input-file stack followed through parentheses (for
//! warnings, which name only a line), `LaTeX`/`Package`/`Class` warnings
//! with their `(pkg)` continuation lines, overfull and underfull boxes,
//! missing files, and the page count. Pure: the glue reads the log and
//! publishes.
//!
//! Paths in a log are the engine's: relative to the folder it ran in (the
//! main file's), or absolute. `Places` turns them into workspace paths; a
//! file outside the workspace (a package) is never a place, so its errors
//! land on the nearest workspace file on the stack.

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub file: String,
    pub line: u32,
    /// `error`, `warning` or `info` (boxes).
    pub severity: &'static str,
    pub message: String,
    /// The `l.<n>` context: the source text up to the error.
    pub context: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub diags: Vec<Diag>,
    /// Files TeX could not find (`siunitx.sty`): a missing package.
    pub missing: Vec<String>,
    /// Problems with no place: a fatal stop with nothing better said.
    pub general: Vec<String>,
    pub pages: Option<u32>,
}

impl Parsed {
    pub fn count(&self, severity: &str) -> usize {
        self.diags.iter().filter(|d| d.severity == severity).count()
    }
}

/// Where the engine ran and what the workspace is, to place its paths.
pub struct Places<'a> {
    /// The main file's folder, workspace-relative (`""` at the root).
    pub dir: &'a str,
    /// The workspace root, absolute, without a trailing `/`.
    pub root: &'a str,
}

impl Places<'_> {
    /// A path the engine printed, as a workspace path (None: outside).
    pub fn place(&self, raw: &str) -> Option<String> {
        let raw = raw.trim();
        let rel = if let Some(abs) = raw.strip_prefix('/') {
            let root = self.root.trim_start_matches('/');
            abs.strip_prefix(root)?.strip_prefix('/')?.to_string()
        } else if self.dir.is_empty() {
            raw.to_string()
        } else {
            format!("{}/{raw}", self.dir)
        };
        normalize(&rel)
    }
}

/// `a/./b/../c` → `a/c`; None if it climbs above the root.
pub fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

pub const DIAGS_MAX: usize = 500;
const MESSAGE_MAX: usize = 1000;

fn clip(text: &str) -> String {
    let text = text.trim();
    if text.len() <= MESSAGE_MAX {
        return text.to_string();
    }
    let mut end = MESSAGE_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// A token right after `(` that is a file the engine opened.
fn opened_file(rest: &str) -> Option<&str> {
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ')' || c == '(' || c == '[' || c == '{')
        .unwrap_or(rest.len());
    let token = &rest[..end];
    let path_like = token.starts_with("./") || token.starts_with('/') || token.starts_with("../");
    let has_ext = token.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && (1..=4).contains(&ext.len())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
    });
    (path_like || has_ext).then_some(token)
}

/// The file stack, followed through a line's parentheses. Each `(` pushes
/// (a file, or nothing for a plain parenthesis) and each `)` pops, so a
/// message's own parentheses balance themselves.
fn follow_stack(line: &str, stack: &mut Vec<Option<String>>) {
    let mut rest = line;
    while let Some(at) = rest.find(['(', ')']) {
        if rest.as_bytes()[at] == b'(' {
            let after = &rest[at + 1..];
            stack.push(opened_file(after).map(str::to_string));
        } else {
            stack.pop();
        }
        rest = &rest[at + 1..];
    }
}

/// `./main.tex:12: message` (the path ends in an extension).
fn file_line_error(line: &str) -> Option<(&str, u32, &str)> {
    let mut search = 0;
    while let Some(i) = line[search..].find(':') {
        let colon = search + i;
        let path = &line[..colon];
        let rest = &line[colon + 1..];
        if let Some(end) = rest.find(": ") {
            if let Ok(n) = rest[..end].parse::<u32>() {
                let ext_ok = path
                    .rsplit_once('.')
                    .is_some_and(|(_, ext)| (1..=4).contains(&ext.len()) && !ext.contains('/'));
                if ext_ok && !path.contains(' ') {
                    return Some((path, n, &rest[end + 2..]));
                }
            }
        }
        search = colon + 1;
    }
    None
}

/// `on input line 12.` anywhere in a message.
fn input_line(message: &str) -> Option<u32> {
    let at = message.find("on input line ")?;
    let digits: String = message[at + 14..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// `` File `siunitx.sty' not found`` (either quote style).
fn missing_file(message: &str) -> Option<String> {
    let at = message.find("File `").or_else(|| message.find("File '"))?;
    let rest = &message[at + 6..];
    let end = rest.find('\'')?;
    let name = &rest[..end];
    (message[at + 6 + end..].contains("not found") && !name.is_empty()).then(|| name.to_string())
}

/// The context line `l.12 \foo` of an error: its number and text.
fn context_line(line: &str) -> Option<(u32, String)> {
    let rest = line.strip_prefix("l.")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let n = digits.parse().ok()?;
    Some((n, rest[digits.len()..].trim().to_string()))
}

pub fn parse(log: &str, places: &Places) -> Parsed {
    let mut out = Parsed::default();
    let mut seen: HashSet<(String, u32, String)> = HashSet::new();
    let mut stack: Vec<Option<String>> = Vec::new();
    let lines: Vec<&str> = log.lines().collect();
    // The nearest workspace file on the stack.
    let current = |stack: &Vec<Option<String>>| -> Option<String> {
        stack.iter().rev().flatten().find_map(|f| places.place(f))
    };
    let mut push = |out: &mut Parsed, d: Diag| {
        if out.diags.len() >= DIAGS_MAX {
            return;
        }
        if seen.insert((d.file.clone(), d.line, d.message.clone())) {
            out.diags.push(d);
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // An error in `-file-line-error` form.
        if let Some((path, n, message)) = file_line_error(line) {
            let (message, next) = with_continuations(&lines, i, message);
            let context = find_context(&lines, next);
            let fatal = message.contains("Emergency stop") || message.contains("==> Fatal error");
            if let Some(name) = missing_file(&message) {
                out.missing.push(name);
            }
            let file = places.place(path).or_else(|| current(&stack));
            match file {
                Some(file) if !fatal => push(
                    &mut out,
                    Diag {
                        file,
                        line: n.max(1),
                        severity: "error",
                        message: clip(&message),
                        context: context.map(|(_, c)| c),
                    },
                ),
                _ => {
                    if fatal && out.count("error") == 0 && out.general.is_empty() {
                        out.general.push(clip(&message));
                    }
                }
            }
            i = next;
            continue;
        }
        // `! Message.` with the place from the `l.<n>` line after it.
        if let Some(message) = line.strip_prefix("! ") {
            let (message, next) = with_continuations(&lines, i, message);
            if let Some(name) = missing_file(&message) {
                out.missing.push(name.clone());
            }
            let fatal = message.contains("Emergency stop") || message.contains("==> Fatal error");
            if !fatal {
                let context = find_context(&lines, next);
                match (current(&stack), context) {
                    (Some(file), Some((n, text))) => push(
                        &mut out,
                        Diag {
                            file,
                            line: n.max(1),
                            severity: "error",
                            message: clip(&message),
                            context: Some(text),
                        },
                    ),
                    (Some(file), None) => push(
                        &mut out,
                        Diag {
                            file,
                            line: 1,
                            severity: "error",
                            message: clip(&message),
                            context: None,
                        },
                    ),
                    (None, _) => out.general.push(clip(&message)),
                }
            }
            i = next;
            continue;
        }
        // Warnings: `LaTeX Warning:`, `Package x Warning:`, `Class x Warning:`.
        if let Some(message) = warning(line) {
            let (message, next) = with_continuations(&lines, i, message);
            let skip = message.starts_with("There were undefined references")
                || message.starts_with("Label(s) may have changed")
                || message.contains("Please (re)run")
                || message.contains("rerun LaTeX");
            if !skip {
                if let Some(file) = current(&stack) {
                    push(
                        &mut out,
                        Diag {
                            file,
                            line: input_line(&message).unwrap_or(1),
                            severity: "warning",
                            message: clip(&message),
                            context: None,
                        },
                    );
                }
            }
            follow_stack(line, &mut stack);
            i = next;
            continue;
        }
        // Boxes: noise until the end, so `info`.
        if line.starts_with("Overfull \\") || line.starts_with("Underfull \\") {
            if let (Some(file), Some(n)) = (current(&stack), box_line(line)) {
                push(
                    &mut out,
                    Diag {
                        file,
                        line: n,
                        severity: "info",
                        message: clip(
                            line.split(" in paragraph")
                                .next()
                                .unwrap_or(line)
                                .split(" detected")
                                .next()
                                .unwrap_or(line),
                        ),
                        context: None,
                    },
                );
            }
            follow_stack(line, &mut stack);
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("Output written on ") {
            out.pages = rest
                .rsplit_once('(')
                .and_then(|(_, p)| p.split_whitespace().next())
                .and_then(|n| n.parse().ok());
        }
        follow_stack(line, &mut stack);
        i += 1;
    }
    out.missing.sort();
    out.missing.dedup();
    out
}

fn warning(line: &str) -> Option<&str> {
    if let Some(m) = line.strip_prefix("LaTeX Warning: ") {
        return Some(m);
    }
    for kind in ["Package ", "Class "] {
        if let Some(rest) = line.strip_prefix(kind) {
            if let Some(at) = rest.find(" Warning: ") {
                if !rest[..at].contains(' ') {
                    return Some(&rest[at + 10..]);
                }
            }
        }
    }
    None
}

/// `… in paragraph at lines 3--5` or `… detected at line 7`.
fn box_line(line: &str) -> Option<u32> {
    let rest = line
        .rsplit_once("at lines ")
        .or_else(|| line.rsplit_once("at line "))?
        .1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A message and the lines that continue it: `(pkg)   more` lines, or
/// plain lines until a blank one for LaTeX's own. The index after it.
fn with_continuations(lines: &[&str], at: usize, first: &str) -> (String, usize) {
    let mut message = first.trim().to_string();
    let mut i = at + 1;
    while i < lines.len() && i < at + 12 {
        let next = lines[i];
        let t = next.trim_start();
        if next.trim().is_empty() {
            break;
        }
        if t.starts_with('(')
            && t.find(')')
                .is_some_and(|c| c < 30 && !t[1..c].contains(' '))
        {
            let text = t[t.find(')').unwrap_or(0) + 1..].trim();
            message.push(' ');
            message.push_str(text);
            i += 1;
            continue;
        }
        // A warning's sentence wraps onto its next line (up to its period).
        if !message.ends_with('.')
            && !next.starts_with("l.")
            && !next.starts_with('!')
            && file_line_error(next).is_none()
            && !next.starts_with("See the")
            && !next.starts_with("Type ")
            && !next.starts_with("For immediate help")
            && !next.starts_with("<")
        {
            message.push(' ');
            message.push_str(next.trim());
            i += 1;
            continue;
        }
        break;
    }
    (message, i)
}

/// The `l.<n>` line within the few lines after an error.
fn find_context(lines: &[&str], from: usize) -> Option<(u32, String)> {
    lines[from.min(lines.len())..]
        .iter()
        .take(14)
        .find_map(|l| context_line(l))
}

/// Where on its line an error is, from its `l.<n>` context (the source up
/// to where TeX stopped): the control sequence it ends with, as 1-based
/// `(column, end_column)`. None when TeX cut the line's start (`...`) or it
/// stopped on something else: the whole line is marked then.
pub fn columns(context: &str) -> Option<(u32, u32)> {
    if context.starts_with("...") {
        return None;
    }
    let chars: Vec<char> = context.chars().collect();
    let slash = chars.iter().rposition(|c| *c == '\\')?;
    let name = &chars[slash + 1..];
    if name.is_empty() || !name.iter().all(|c| c.is_ascii_alphabetic() || *c == '@') {
        return None;
    }
    Some((slash as u32 + 1, chars.len() as u32 + 1))
}

/// The `.fls` recorder file's inputs inside the workspace (the watch set,
/// and how a part finds its document): `INPUT` lines, relative to the folder
/// the engine ran in, never the build folder's own files.
pub fn fls_inputs(fls: &str, places: &Places, out_dir: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut inputs = Vec::new();
    for line in fls.lines() {
        let Some(path) = line.strip_prefix("INPUT ") else {
            continue;
        };
        if path.starts_with(out_dir) {
            continue;
        }
        if let Some(p) = places.place(path) {
            if seen.insert(p.clone()) {
                inputs.push(p);
            }
        }
    }
    inputs
}

/// Biber's or BibTeX's own problems, from its `.blg`: `(severity, message)`.
pub fn blg(text: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(at) = line.find("> ERROR - ") {
            out.push(("error", clip(&line[at + 10..])));
        } else if let Some(at) = line.find("> WARN - ") {
            out.push(("warning", clip(&line[at + 9..])));
        } else if let Some(m) = line.strip_prefix("Warning--") {
            out.push(("warning", clip(m)));
        } else if line.starts_with("I found no ") || line.starts_with("I couldn't open") {
            out.push(("error", clip(line)));
        }
        if out.len() >= 50 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/home/u/ws";

    fn at_root() -> Places<'static> {
        Places {
            dir: "",
            root: ROOT,
        }
    }

    fn errors(p: &Parsed) -> Vec<(String, u32, String)> {
        p.diags
            .iter()
            .filter(|d| d.severity == "error")
            .map(|d| (d.file.clone(), d.line, d.message.clone()))
            .collect()
    }

    #[test]
    fn places_are_workspace_paths() {
        let p = Places {
            dir: "docs",
            root: ROOT,
        };
        assert_eq!(p.place("./main.tex").as_deref(), Some("docs/main.tex"));
        assert_eq!(p.place("../fig/a.tex").as_deref(), Some("fig/a.tex"));
        assert_eq!(p.place("/home/u/ws/x/y.tex").as_deref(), Some("x/y.tex"));
        assert_eq!(
            p.place("/opt/texlive/texmf-dist/tex/latex/base/article.cls"),
            None
        );
        assert_eq!(p.place("../../outside.tex"), None);
    }

    #[test]
    fn errors_in_each_file_with_their_context() {
        let p = parse(include_str!("../tests/logs/errors.log"), &at_root());
        assert_eq!(
            errors(&p),
            vec![
                ("main.tex".into(), 6, "Undefined control sequence.".into()),
                (
                    "chapters/one.tex".into(),
                    3,
                    "Undefined control sequence.".into()
                ),
                ("main.tex".into(), 10, "Missing $ inserted.".into()),
            ]
        );
        assert_eq!(p.diags[0].context.as_deref(), Some("Hello \\unit"));
        let warnings: Vec<_> = p
            .diags
            .iter()
            .filter(|d| d.severity == "warning")
            .map(|d| (d.file.as_str(), d.line, d.message.as_str()))
            .collect();
        assert_eq!(
            warnings,
            vec![
                (
                    "main.tex",
                    6,
                    "Reference `sec:missing' on page 1 undefined on input line 6."
                ),
                (
                    "main.tex",
                    6,
                    "Citation `nokey' on page 1 undefined on input line 6."
                ),
            ]
        );
        assert_eq!(p.pages, Some(1));
        assert!(p.missing.is_empty());
    }

    #[test]
    fn a_missing_package_is_named() {
        let p = parse(include_str!("../tests/logs/missing-pkg.log"), &at_root());
        assert_eq!(p.missing, vec!["nosuchpackagexyz.sty".to_string()]);
        let e = errors(&p);
        assert_eq!(e.len(), 1, "{:?}", p.diags);
        assert_eq!(e[0].0, "main.tex");
        assert_eq!(e[0].1, 5);
        assert!(e[0].2.contains("nosuchpackagexyz.sty"), "{}", e[0].2);
        assert_eq!(p.pages, None);
    }

    #[test]
    fn a_clean_build_has_only_boxes_in_its_part() {
        let p = parse(include_str!("../tests/logs/good.log"), &at_root());
        assert_eq!(p.count("error"), 0);
        assert_eq!(p.count("warning"), 0);
        let boxes: Vec<_> = p
            .diags
            .iter()
            .map(|d| (d.file.as_str(), d.line, d.severity))
            .collect();
        assert_eq!(
            boxes,
            vec![("sec/part.tex", 1, "info"), ("sec/part.tex", 3, "info")]
        );
        assert!(p.diags[0]
            .message
            .starts_with("Overfull \\hbox (128.06697pt too wide)"));
        assert_eq!(p.pages, Some(1));
    }

    #[test]
    fn a_package_error_joins_its_lines_once() {
        let p = parse(include_str!("../tests/logs/xe.log"), &at_root());
        let e = errors(&p);
        assert!(
            e[0].2.starts_with(
                "Package fontspec Error: The font \"NoSuchFontAtAll\" cannot be found"
            ),
            "{}",
            e[0].2
        );
        // Repeated three times in the log; said once.
        assert_eq!(
            e.iter()
                .filter(|x| x.2.starts_with("Package fontspec Error"))
                .count(),
            1
        );
        assert!(e.iter().any(|x| x
            == &(
                "xe.tex".to_string(),
                6,
                "Undefined control sequence.".to_string()
            )));
    }

    #[test]
    fn lualatex_and_biblatex() {
        let p = parse(include_str!("../tests/logs/lua.log"), &at_root());
        assert_eq!(
            errors(&p),
            vec![("lua.tex".into(), 3, "Undefined control sequence.".into())]
        );
        let p = parse(include_str!("../tests/logs/bib.log"), &at_root());
        let w: Vec<_> = p.diags.iter().map(|d| d.message.as_str()).collect();
        assert!(
            w.contains(&"Citation 'missingkey' on page 1 undefined on input line 5."),
            "{w:?}"
        );
        assert!(!w.iter().any(|m| m.contains("rerun")), "{w:?}");
    }

    #[test]
    fn biber_says_what_broke_in_the_bib() {
        let b = blg(include_str!("../tests/logs/bib.blg"));
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].0, "error");
        assert!(b[0].1.contains("syntax error"), "{}", b[0].1);
        assert_eq!(
            blg("Warning--I didn't find a database entry for \"x\"\n")[0].0,
            "warning"
        );
    }

    #[test]
    fn the_recorder_file_gives_the_inputs() {
        let fls = include_str!("../tests/logs/good.fls");
        assert_eq!(
            fls_inputs(fls, &at_root(), "/home/u/ws/o-good"),
            vec![
                "good.tex".to_string(),
                "o-good/good.aux".to_string(),
                "sec/part.tex".to_string()
            ]
        );
        // The real build folder is absolute and outside the workspace.
        let real = "PWD /home/u/ws\nINPUT main.tex\nINPUT /cache/x/main.aux\nINPUT ./ch/a.tex\nINPUT /opt/texlive/a.sty\n";
        assert_eq!(
            fls_inputs(real, &at_root(), "/cache/x"),
            vec!["main.tex".to_string(), "ch/a.tex".to_string()]
        );
    }

    #[test]
    fn an_error_marks_its_control_sequence() {
        assert_eq!(columns("Hello \\unit"), Some((7, 12)));
        assert_eq!(columns("\\undefinedmacroinchapter"), Some((1, 25)));
        assert_eq!(columns("...long line \\x"), None);
        assert_eq!(columns("\\end{document}"), None);
        assert_eq!(columns("plain"), None);
    }

    #[test]
    fn the_stack_balances_plain_parentheses() {
        let log = "(./main.tex (see the transcript) (./ch/a.tex\nLaTeX Warning: Reference `x' on page 1 undefined on input line 4.\n)\nLaTeX Warning: Reference `y' on page 1 undefined on input line 9.\n)\n";
        let p = parse(log, &at_root());
        let w: Vec<_> = p.diags.iter().map(|d| (d.file.as_str(), d.line)).collect();
        assert_eq!(w, vec![("ch/a.tex", 4), ("main.tex", 9)]);
    }
}
