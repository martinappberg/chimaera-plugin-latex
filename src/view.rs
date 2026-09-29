//! The document view as a `ui/1` tree, from what the glue knows of a
//! document. Pure (tested natively): the look lives here.
//!
//! ```text
//! [● 1 error] 2 pages · pdfLaTeX · TinyTeX   [Split|Source|PDF] [Ask agent to fix] [Log] [Save PDF] [Build]
//! ⚠ notice, only when something needs the user (a missing package, a stop)
//! editor                              │ PDF
//!                                     │ PROBLEMS  1 error
//! ```

use chimaera_plugin_api::serde_json::{json, Value};

/// What the view shows of one document.
pub struct Doc<'a> {
    /// The file the view opened (a part, or the main file).
    pub file: &'a str,
    pub main: &'a str,
    /// `split`, `source` or `pdf`.
    pub layout: &'a str,
    pub narrow: bool,
    /// What runs now ("Building…", "Installing siunitx…").
    pub busy: Option<String>,
    /// The last build: `new`, `ok`, `errors`, `failed`.
    pub state: &'a str,
    pub errors: u64,
    pub warnings: u64,
    pub pages: Option<u64>,
    pub ms: Option<u64>,
    pub engine: Option<&'a str>,
    /// `path` (the host's TeX Live) or `tool:tinytex`.
    pub from: Option<&'a str>,
    pub has_pdf: bool,
    pub has_log: bool,
    /// No latexmk anywhere: the install offer.
    pub no_engine: bool,
    /// `{tone, title, text, offer?, log?}`: something the user should act on.
    pub notice: Option<&'a Value>,
    /// The document's build folder (`output:<key>/`).
    pub key: String,
    pub stem: &'a str,
    pub pdf: String,
    pub beside: String,
}

fn engine_name(engine: &str) -> &'static str {
    match engine {
        "xelatex" => "XeLaTeX",
        "lualatex" => "LuaLaTeX",
        _ => "pdfLaTeX",
    }
}

fn plural(n: u64, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// The status pill and the detail beside it.
pub fn status(d: &Doc) -> Value {
    let (state, text) = if let Some(busy) = &d.busy {
        ("busy", busy.clone())
    } else if d.no_engine {
        ("warn", "No TeX Live".to_string())
    } else {
        match d.state {
            "ok" => ("ok", "Built".to_string()),
            "errors" => ("bad", plural(d.errors, "error")),
            "failed" => ("bad", "Build stopped".to_string()),
            _ => ("idle", "Not built yet".to_string()),
        }
    };
    let mut detail: Vec<String> = Vec::new();
    if d.no_engine && d.busy.is_none() {
        // Nothing of an older build: it would read as if one just ran.
        return json!({"type": "status", "state": state, "text": text,
                      "detail": "latexmk wasn't found on this host"});
    }
    if d.busy.is_none() && d.state == "ok" {
        if let Some(p) = d.pages {
            detail.push(plural(p, "page"));
        }
        if d.warnings > 0 {
            detail.push(plural(d.warnings, "warning"));
        }
        if let Some(ms) = d.ms {
            detail.push(format!("{:.1} s", ms as f64 / 1000.0));
        }
    } else if d.busy.is_none() && d.state == "errors" && d.warnings > 0 {
        detail.push(plural(d.warnings, "warning"));
    }
    if let Some(e) = d.engine {
        detail.push(engine_name(e).to_string());
    }
    match d.from {
        Some("tool:tinytex") => detail.push("TinyTeX".into()),
        Some("path") => detail.push("your TeX Live".into()),
        _ => {}
    }
    if d.main != d.file {
        detail.push(format!("part of {}", d.main));
    }
    json!({"type": "status", "state": state, "text": text, "detail": detail.join(" · ")})
}

fn button(label: &str, action: &str, payload: Value, icon: &str) -> Value {
    json!({"type": "button", "label": label, "action": action, "payload": payload, "icon": icon})
}

fn toolbar(d: &Doc) -> Value {
    let busy = d.busy.is_some();
    let mut right: Vec<Value> = Vec::new();
    if !d.narrow {
        right.push(
            json!({"type": "segmented", "name": "layout", "label": "Layout", "value": d.layout,
            "action": "layout", "payload": {"file": d.file},
            "options": [
                {"value": "split", "label": "Split", "title": "Source and PDF side by side"},
                {"value": "source", "label": "Source", "title": "The source only"},
                {"value": "pdf", "label": "PDF", "title": "The PDF only"},
            ]}),
        );
    }
    if d.errors > 0 && !busy && d.state == "errors" {
        let mut b = button(
            "Ask agent to fix",
            "ask-agent",
            json!({"file": d.main, "text": format!(
                "Fix the {} in {} (compile_latex lists them with the source text at each)",
                plural(d.errors, "compile error"), d.main
            )}),
            "bolt",
        );
        b["title"] = json!("Put the errors in front of an agent");
        right.push(b);
    }
    if d.has_log {
        let mut b = button(
            "Log",
            "open-file",
            json!({"file": format!("output:{}/{}.log", d.key, d.stem)}),
            "file",
        );
        b["title"] = json!("The full LaTeX log");
        right.push(b);
    }
    if d.has_pdf {
        let mut b = button(
            "Save PDF",
            "save-to-workspace",
            json!({"from": d.pdf, "to": d.beside}),
            "download",
        );
        b["title"] = json!(format!("Copy the PDF into the project as {}", d.beside));
        right.push(b);
    }
    let mut build = button(
        if busy { "Building…" } else { "Build" },
        "build",
        json!({"main": d.main}),
        "play",
    );
    // Never held for a missing TeX Live: the user may just have loaded one.
    build["disabled"] = json!(busy);
    right.push(build);
    json!({"type": "row", "align": "between", "gap": "small", "children": [
        status(d),
        {"type": "row", "gap": "small", "children": right},
    ]})
}

fn no_tex_text() -> &'static str {
    "Chimaera looked on the PATH your terminals get, including your environment prelude. On a cluster, load yours in Environment settings (often: module load texlive) and build again, or install TinyTeX, a current TeX Live, into this plugin's own folder. Nothing else on the system changes."
}

fn install_button() -> Value {
    json!({"type": "button", "label": "Install TeX Live (TinyTeX, 152 MB)", "action": "install-tool",
           "payload": {"tool": "tinytex"}, "icon": "download", "tone": "accent"})
}

fn notice(d: &Doc) -> Option<Value> {
    if d.no_engine && d.has_pdf && d.busy.is_none() {
        return Some(
            json!({"type": "callout", "tone": "warn", "title": "No TeX Live on this host",
            "text": no_tex_text(), "actions": [install_button()]}),
        );
    }
    let n = d.notice?;
    let mut actions = Vec::new();
    if let Some(p) = n["offer"].as_str() {
        let mut b = button(
            &format!("Install {p} anyway"),
            "install-package",
            json!({"main": d.main, "package": p}),
            "download",
        );
        b["title"] = json!("Trust the mirror's checksums alone");
        actions.push(b);
    }
    if n["log"].as_bool() == Some(true) && d.has_log {
        actions.push(button(
            "Open log",
            "open-file",
            json!({"file": format!("output:{}/{}.log", d.key, d.stem)}),
            "file",
        ));
    }
    let mut c = json!({"type": "callout", "tone": n["tone"].as_str().unwrap_or("warn"),
                       "text": n["text"].as_str().unwrap_or("")});
    if let Some(t) = n["title"].as_str() {
        c["title"] = json!(t);
    }
    if !actions.is_empty() {
        c["actions"] = json!(actions);
    }
    Some(c)
}

fn problems(d: &Doc) -> Value {
    json!({"type": "diagnostics", "key": d.key, "quiet": true, "compact": true})
}

/// The result side: the PDF (kept while a new build runs), or why there is
/// none yet.
fn result(d: &Doc) -> Value {
    if d.no_engine && !d.has_pdf && d.busy.is_none() {
        return json!({"type": "empty", "title": "No TeX Live on this host", "text": no_tex_text(),
            "action": {"label": "Install TeX Live (TinyTeX, 152 MB)", "action": "install-tool", "payload": {"tool": "tinytex"}}});
    }
    if d.has_pdf {
        return json!({"type": "stack", "gap": "small", "children": [{"type": "pdf", "src": d.pdf}, problems(d)]});
    }
    if let Some(busy) = &d.busy {
        return json!({"type": "stack", "gap": "small", "children": [
            {"type": "empty", "title": busy, "text": "The PDF appears here when the first build finishes."},
            problems(d),
        ]});
    }
    json!({"type": "stack", "gap": "small", "children": [
        {"type": "empty", "title": "No PDF yet",
         "text": if d.state == "new" { "Build the document to see it here." } else { "The last build made no PDF: its problems are below." },
         "action": {"label": "Build", "action": "build", "payload": {"main": d.main}}},
        problems(d),
    ]})
}

pub fn tree(d: &Doc) -> Value {
    let editor = json!({"type": "editor", "path": d.file});
    let body = if d.narrow {
        json!({"type": "tabs", "tabs": [
            {"title": "PDF", "children": [result(d)]},
            {"title": "Source", "children": [editor]},
        ]})
    } else {
        match d.layout {
            "source" => json!({"type": "stack", "gap": "small", "children": [editor, problems(d)]}),
            "pdf" => result(d),
            _ => json!({"type": "split", "ratio": 0.5, "children": [editor, result(d)]}),
        }
    };
    let mut children = vec![toolbar(d)];
    if let Some(n) = notice(d) {
        children.push(n);
    }
    children.push(body);
    json!({"ui": "1", "root": {"type": "stack", "gap": "small", "children": children}})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc<'a>(state: &'a str) -> Doc<'a> {
        Doc {
            file: "main.tex",
            main: "main.tex",
            layout: "split",
            narrow: false,
            busy: None,
            state,
            errors: 0,
            warnings: 0,
            pages: None,
            ms: None,
            engine: Some("pdflatex"),
            from: Some("tool:tinytex"),
            has_pdf: false,
            has_log: false,
            no_engine: false,
            notice: None,
            key: "k-main".into(),
            stem: "main",
            pdf: "output:k-main/main.pdf".into(),
            beside: "main.pdf".into(),
        }
    }

    fn find<'v>(v: &'v Value, kind: &str, out: &mut Vec<&'v Value>) {
        if v["type"] == kind {
            out.push(v);
        }
        match v {
            Value::Object(o) => o.values().for_each(|c| find(c, kind, out)),
            Value::Array(a) => a.iter().for_each(|c| find(c, kind, out)),
            _ => {}
        }
    }

    fn all<'v>(v: &'v Value, kind: &str) -> Vec<&'v Value> {
        let mut out = Vec::new();
        find(v, kind, &mut out);
        out
    }

    #[test]
    fn a_good_build_says_so_with_its_facts() {
        let mut d = doc("ok");
        d.pages = Some(14);
        d.ms = Some(1234);
        d.has_pdf = true;
        d.has_log = true;
        let t = tree(&d);
        let s = all(&t, "status")[0];
        assert_eq!(s["state"], "ok");
        assert_eq!(s["text"], "Built");
        assert_eq!(s["detail"], "14 pages · 1.2 s · pdfLaTeX · TinyTeX");
        let labels: Vec<_> = all(&t, "button")
            .iter()
            .map(|b| b["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, vec!["Log", "Save PDF", "Build"]);
        assert_eq!(all(&t, "pdf").len(), 1);
        assert_eq!(all(&t, "diagnostics")[0]["key"], "k-main");
        assert!(all(&t, "callout").is_empty());
    }

    #[test]
    fn errors_offer_the_agent() {
        let mut d = doc("errors");
        d.errors = 2;
        d.warnings = 1;
        let t = tree(&d);
        assert_eq!(all(&t, "status")[0]["text"], "2 errors");
        let ask = all(&t, "button")
            .into_iter()
            .find(|b| b["action"] == "ask-agent")
            .unwrap();
        assert!(ask["payload"]["text"]
            .as_str()
            .unwrap()
            .contains("2 compile errors in main.tex"));
    }

    #[test]
    fn building_keeps_the_pdf_and_holds_the_button() {
        let mut d = doc("ok");
        d.busy = Some("Building…".into());
        d.has_pdf = true;
        let t = tree(&d);
        assert_eq!(all(&t, "status")[0]["state"], "busy");
        let build = all(&t, "button")
            .into_iter()
            .find(|b| b["action"] == "build")
            .unwrap();
        assert_eq!(build["disabled"], true);
        assert_eq!(build["label"], "Building…");
        assert_eq!(all(&t, "pdf").len(), 1);
    }

    #[test]
    fn no_tex_offers_the_install() {
        let mut d = doc("new");
        d.no_engine = true;
        d.from = None;
        let t = tree(&d);
        let e = all(&t, "empty")[0];
        assert_eq!(e["action"]["action"], "install-tool");
        assert_eq!(all(&t, "status")[0]["text"], "No TeX Live");
        let build = all(&t, "button")
            .into_iter()
            .find(|b| b["action"] == "build")
            .unwrap();
        assert_eq!(build["disabled"], false);
        // An older PDF stays in view; the offer moves to a notice, and the
        // status says nothing of the old build.
        let mut d = doc("ok");
        d.no_engine = true;
        d.has_pdf = true;
        d.pages = Some(3);
        let t = tree(&d);
        assert_eq!(all(&t, "pdf").len(), 1);
        assert_eq!(
            all(&t, "callout")[0]["actions"][0]["action"],
            "install-tool"
        );
        assert_eq!(
            all(&t, "status")[0]["detail"],
            "latexmk wasn't found on this host"
        );
    }

    #[test]
    fn a_notice_carries_its_fix() {
        let n = json!({"tone": "warn", "title": "Couldn't install siunitx", "text": "no gpg", "offer": "siunitx", "log": true});
        let mut d = doc("errors");
        d.notice = Some(&n);
        d.has_log = true;
        let t = tree(&d);
        let c = all(&t, "callout")[0];
        let acts: Vec<_> = c["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["action"].as_str().unwrap())
            .collect();
        assert_eq!(acts, vec!["install-package", "open-file"]);
    }

    #[test]
    fn layouts_and_narrow() {
        let mut d = doc("ok");
        d.layout = "source";
        let t = tree(&d);
        assert!(all(&t, "split").is_empty());
        assert_eq!(all(&t, "editor").len(), 1);
        d.layout = "pdf";
        assert!(all(&tree(&d), "editor").is_empty());
        d.narrow = true;
        let t = tree(&d);
        assert_eq!(all(&t, "tabs").len(), 1);
        assert!(all(&t, "segmented").is_empty());
        let mut part = doc("ok");
        part.file = "ch/a.tex";
        assert!(all(&tree(&part), "status")[0]["detail"]
            .as_str()
            .unwrap()
            .ends_with("part of main.tex"));
    }
}
