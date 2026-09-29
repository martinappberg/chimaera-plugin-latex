//! A LaTeX document: its main file, engine, build folder, and the words its
//! state reads as. Pure, so it runs natively under `cargo test`.

use std::collections::BTreeMap;

fn fnv1a(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// `ch/report.tex` → `report`.
pub fn stem(main: &str) -> &str {
    let name = main.rsplit('/').next().unwrap_or(main);
    name.strip_suffix(".tex")
        .or_else(|| name.strip_suffix(".ltx"))
        .unwrap_or(name)
}

/// The main file's folder (`""` at the root) and its name.
pub fn split(main: &str) -> (&str, &str) {
    main.rsplit_once('/').unwrap_or(("", main))
}

/// The document's build folder in the output folder: `3f9a2c1e-report`.
pub fn key(main: &str) -> String {
    let safe: String = stem(main)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    format!("{:08x}-{safe}", fnv1a(main) as u32)
}

pub fn pdf(main: &str) -> String {
    format!("output:{}/{}.pdf", key(main), stem(main))
}

/// Where **Save PDF beside source** puts it.
pub fn beside(main: &str) -> String {
    match split(main) {
        ("", _) => format!("{}.pdf", stem(main)),
        (dir, _) => format!("{dir}/{}.pdf", stem(main)),
    }
}

pub fn is_tex(path: &str) -> bool {
    path.ends_with(".tex") || path.ends_with(".ltx")
}

/// A `% !TEX <name> = <value>` magic comment in a file's head (the first
/// 40 lines; either case, `TeX` too).
pub fn magic<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    for line in head.lines().take(40) {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix('%') else {
            continue;
        };
        let rest = rest.trim_start();
        if rest.len() < 5 || !rest[..5].eq_ignore_ascii_case("!tex ") {
            continue;
        }
        let rest = rest[5..].trim_start();
        let Some((key, value)) = rest.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(name) {
            let v = value.trim();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// The engine: the file's `% !TEX program`, else the workspace's setting,
/// else pdfLaTeX. A value latexmk doesn't know is ignored.
pub fn engine(head: &str, setting: Option<&str>) -> &'static str {
    let pick = |v: &str| match v.to_ascii_lowercase().as_str() {
        "pdflatex" => Some("pdflatex"),
        "xelatex" => Some("xelatex"),
        "lualatex" => Some("lualatex"),
        _ => None,
    };
    magic(head, "program")
        .and_then(pick)
        .or_else(|| setting.and_then(pick))
        .unwrap_or("pdflatex")
}

/// latexmk's flag for an engine.
pub fn engine_flag(engine: &str) -> &'static str {
    match engine {
        "xelatex" => "-xelatex",
        "lualatex" => "-lualatex",
        _ => "-pdf",
    }
}

/// Whether a file's head starts a document (a main file).
pub fn is_document(head: &str) -> bool {
    head.lines().any(|l| {
        let t = l.trim_start();
        !t.starts_with('%') && t.contains("\\documentclass")
    })
}

/// The main file for `file`: its `% !TEX root`, else a document whose last
/// build read it, else itself (a part with neither builds as one, and its
/// errors say why).
pub fn main_for(file: &str, head: &str, inputs: &BTreeMap<String, Vec<String>>) -> String {
    if let Some(root) = magic(head, "root") {
        let (dir, _) = split(file);
        let joined = if dir.is_empty() {
            root.to_string()
        } else {
            format!("{dir}/{root}")
        };
        if let Some(main) = super::log::normalize(&joined) {
            if is_tex(&main) {
                return main;
            }
        }
    }
    if let Some(main) = inputs
        .iter()
        .find(|(main, read)| main.as_str() != file && read.iter().any(|r| r == file))
        .map(|(main, _)| main.clone())
    {
        return main;
    }
    file.to_string()
}

/// `tlmgr search --global --file /siunitx.sty`: the package that holds the
/// file (a line `name:` followed by indented paths ending in it).
pub fn package_for(search: &str, file: &str) -> Option<String> {
    let mut package: Option<&str> = None;
    for line in search.lines() {
        if !line.starts_with(char::is_whitespace) {
            package = line
                .strip_suffix(':')
                .filter(|p| !p.is_empty() && !p.contains(' ') && !p.starts_with("tlmgr"));
            continue;
        }
        let path = line.trim();
        if let Some(p) = package {
            if path.rsplit('/').next() == Some(file) {
                return Some(p.to_string());
            }
        }
    }
    None
}

/// A package name tlmgr may be asked for (never an option).
pub fn valid_package(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn result_words(
    errors: usize,
    warnings: usize,
    pages: Option<u32>,
    duration_ms: u64,
) -> String {
    let secs = duration_ms as f64 / 1000.0;
    let pages = pages.map_or(String::new(), |p| {
        format!(" · {p} page{}", plural(p as usize))
    });
    match (errors, warnings) {
        (0, 0) => format!("built in {secs:.1} s{pages}"),
        (0, w) => format!("built in {secs:.1} s{pages} · {w} warning{}", plural(w)),
        (e, 0) => format!("{e} error{}", plural(e)),
        (e, w) => format!("{e} error{} · {w} warning{}", plural(e), plural(w)),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// tlmgr's complaint in a line: its own path prefix dropped, and an
/// unreachable package list said plainly.
pub fn tlmgr_words(stderr: &str) -> String {
    if stderr.contains("could not get texlive.tlpdb") || stderr.contains("TLPDB::from_file") {
        return "the TeX Live package list couldn't be downloaded (offline, or the mirror is blocked)".into();
    }
    let line = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line = line.rsplit_once("tlmgr: ").map_or(line, |(_, rest)| rest);
    line.chars().take(200).collect()
}

pub fn not_found(err: &str) -> bool {
    err.contains("was not found") || err.contains("is not installed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_comments() {
        let head = "% !TEX program = XeLaTeX\n%!TeX root = ../main.tex\n\\documentclass{article}\n";
        assert_eq!(magic(head, "program"), Some("XeLaTeX"));
        assert_eq!(engine(head, None), "xelatex");
        assert_eq!(engine("", Some("lualatex")), "lualatex");
        assert_eq!(
            engine("% !TEX program = tectonic\n", Some("nope")),
            "pdflatex"
        );
        assert_eq!(engine_flag("xelatex"), "-xelatex");
        assert!(is_document(head));
        assert!(!is_document("% \\documentclass{x}\n\\section{A}\n"));
    }

    #[test]
    fn a_part_finds_its_document() {
        let mut inputs = BTreeMap::new();
        inputs.insert(
            "thesis.tex".to_string(),
            vec!["thesis.tex".to_string(), "ch/a.tex".to_string()],
        );
        assert_eq!(
            main_for("ch/b.tex", "%!TEX root = ../thesis.tex\n", &inputs),
            "thesis.tex"
        );
        assert_eq!(main_for("ch/a.tex", "", &inputs), "thesis.tex");
        assert_eq!(main_for("other.tex", "", &inputs), "other.tex");
        assert_eq!(
            main_for("a.tex", "% !TEX root = ../../up.tex\n", &inputs),
            "a.tex"
        );
    }

    #[test]
    fn keys_and_places() {
        assert_eq!(stem("ch/report.tex"), "report");
        assert_eq!(beside("ch/report.tex"), "ch/report.pdf");
        assert_eq!(beside("report.ltx"), "report.pdf");
        assert_ne!(key("a/r.tex"), key("b/r.tex"));
        assert_eq!(split("a/b/c.tex"), ("a/b", "c.tex"));
    }

    #[test]
    fn tlmgr_names_the_package() {
        let out = "tlmgr: package repository https://mirror.example/tlnet (verified)\nsiunitx:\n\ttexmf-dist/tex/latex/siunitx/siunitx.sty\nsiunitx-legacy:\n\ttexmf-dist/tex/latex/siunitx/siunitx-v2.sty\n";
        assert_eq!(package_for(out, "siunitx.sty").as_deref(), Some("siunitx"));
        assert_eq!(package_for(out, "nothere.sty"), None);
        assert!(valid_package("siunitx"));
        assert!(!valid_package("--all"));
        assert!(!valid_package("a b"));
        assert_eq!(
            tlmgr_words("/x/.TinyTeX/bin/x86_64-linux/tlmgr: TLPDB::from_file could not get texlive.tlpdb from: https://m/tlpkg/texlive.tlpdb\n"),
            "the TeX Live package list couldn't be downloaded (offline, or the mirror is blocked)"
        );
        assert_eq!(
            tlmgr_words("/x/tlmgr: package foo not present in repository.\n"),
            "package foo not present in repository."
        );
    }

    #[test]
    fn words() {
        assert_eq!(
            result_words(0, 0, Some(14), 1234),
            "built in 1.2 s · 14 pages"
        );
        assert_eq!(result_words(2, 1, None, 1), "2 errors · 1 warning");
    }
}
