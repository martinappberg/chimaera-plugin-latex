//! LaTeX for Chimaera (plugin API 0.2): `.tex` opens as source beside its
//! PDF; a save, an agent's edit or Build runs latexmk as a job the host
//! limits; the log becomes `diagnostics/1` (editor marks, the problems list)
//! and `output/1` (the status); a package missing from the plugin's TinyTeX
//! is looked up and installed with tlmgr, then the build reruns once;
//! `compile_latex` lets an agent build and read the errors in one call.
//!
//! latexmk always runs with `-norc` (a project `latexmkrc` is Perl, never
//! run unasked), nonstop, `-file-line-error`, the recorder and SyncTeX on,
//! into the plugin's output folder; TeX's own restricted shell escape stays
//! as the site set it.
//!
//! State (the host's, per workspace, 64 KiB): `builds` (per main file),
//! `jobs` (a job's main file and what it was for), `inputs` (per main file:
//! what its last build read: the watch set, and how a part finds its
//! document).

use std::collections::BTreeMap;

use chimaera_plugin_api::serde_json::{json, Map, Value};
use chimaera_plugin_api::{
    host, platform, Context, Event, JobEnd, Level, Plugin, ToolDef, ToolResult,
};

pub mod doc;
pub mod log;
pub mod view;

struct Latex;

type Obj = Map<String, Value>;

const DOCS_KEPT: usize = 12;
const JOBS_KEPT: usize = 32;
const INPUTS_KEPT: usize = 120;
const WATCH_MAX: usize = 256;
const OPEN_MS: u64 = 10 * 60 * 1000;
/// How much of a log is read (1 MiB a call, 16 MiB in all).
const LOG_CHUNK: u32 = 1 << 20;
const LOG_MAX: u64 = 16 << 20;
const HEAD: u32 = 16 << 10;

fn setting_bool(cx: &Context, key: &str, default: bool) -> bool {
    platform::setting(cx, key)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

fn get(cx: &Context, key: &str) -> Obj {
    host::state_get(cx, key)
        .ok()
        .flatten()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn put(cx: &Context, key: &str, value: Value) {
    if let Err(err) = host::state_put(cx, key, &value) {
        host::log(Level::Warn, &format!("latex: state {key} not kept: {err}"));
    }
}

fn save_builds(cx: &Context, mut map: Obj) {
    while map.len() > DOCS_KEPT {
        let oldest = map
            .iter()
            .min_by_key(|(_, v)| v["touched"].as_u64().unwrap_or(0))
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                map.remove(&k);
            }
            None => break,
        }
    }
    put(cx, "builds", Value::Object(map));
}

fn inputs(cx: &Context) -> BTreeMap<String, Vec<String>> {
    get(cx, "inputs")
        .into_iter()
        .map(|(k, v)| {
            let list = v
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            (k, list)
        })
        .collect()
}

fn head(cx: &Context, path: &str) -> String {
    host::read(cx, path, HEAD)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

fn main_for(cx: &Context, file: &str) -> String {
    doc::main_for(file, &head(cx, file), &inputs(cx))
}

fn remember_job(cx: &Context, job: &str, what: Value) {
    let mut jobs = get(cx, "jobs");
    jobs.insert(job.to_string(), what);
    while jobs.len() > JOBS_KEPT {
        let first = jobs.keys().next().cloned();
        if let Some(k) = first {
            jobs.remove(&k);
        }
    }
    put(cx, "jobs", Value::Object(jobs));
}

fn wall(cx: &Context) -> u64 {
    platform::setting(cx, "timeout_s")
        .and_then(|v| v.as_u64())
        .unwrap_or(180)
}

/// The workspace root and the output folder, absolute, no trailing `/`.
fn roots(cx: &Context) -> (String, String) {
    let r = platform::roots(cx);
    (
        r.workspace.trim_end_matches('/').to_string(),
        r.output.trim_end_matches('/').to_string(),
    )
}

/// Start a build of `main` (or mark one pending behind the running one).
fn build(cx: &Context, main: &str, by: &str) -> Result<String, String> {
    let mut all = get(cx, "builds");
    let entry = all.entry(main.to_string()).or_insert_with(|| json!({}));
    if let Some(job) = entry["job"].as_str().map(str::to_string) {
        entry["pending"] = json!(true);
        save_builds(cx, all);
        return Ok(job);
    }
    let engine = doc::engine(
        &head(cx, main),
        platform::setting(cx, "engine")
            .as_ref()
            .and_then(Value::as_str),
    );
    let (dir, name) = doc::split(main);
    let key = doc::key(main);
    platform::output_write(cx, &format!("{key}/.keep"), b"")?;
    let (_, output) = roots(cx);
    let out = format!("{output}/{key}");
    let mut spec = json!({
        "program": "latexmk",
        "args": [
            doc::engine_flag(engine),
            "-interaction=nonstopmode",
            "-file-line-error",
            "-synctex=1",
            "-recorder",
            format!("-outdir={out}"),
            "-norc",
            name,
        ],
        // TeX stops wrapping its log (the parser's worst enemy), and may
        // write into the absolute build folder.
        "env": {
            "max_print_line": "10000",
            "error_line": "254",
            "half_error_line": "238",
            "TEXMFOUTPUT": out,
        },
        "label": format!("LaTeX: {main}"),
        "priority": by,
        "wall_s": wall(cx),
    });
    if !dir.is_empty() {
        spec["cwd"] = json!(dir);
    }
    if setting_bool(cx, "use_plugin_tinytex", false) {
        spec["prefer"] = json!("tool:tinytex");
    }
    let now = host::now_ms();
    let entry = all.entry(main.to_string()).or_insert_with(|| json!({}));
    entry["touched"] = json!(now);
    match platform::job_start(cx, &spec) {
        Ok(job) => {
            if let Some(o) = entry.as_object_mut() {
                o.insert("job".into(), json!(job));
                o.insert("pending".into(), json!(false));
                o.insert("by".into(), json!(by));
                o.insert("engine".into(), json!(engine));
                o.remove("missing");
                o.remove("notice");
            }
            save_builds(cx, all);
            remember_job(cx, &job, json!({"main": main, "kind": "build"}));
            let _ = platform::publish(
                cx,
                "output/1",
                &key,
                &json!({"source": main, "output": doc::pdf(main), "state": "building", "label": "building…"}),
            );
            platform::invalidate(cx, "document");
            Ok(job)
        }
        Err(err) => {
            if doc::not_found(&err) {
                entry["missing"] = json!(err);
            }
            entry["label"] = json!(err);
            save_builds(cx, all);
            platform::invalidate(cx, "document");
            Err(err)
        }
    }
}

/// An output file whole, up to `LOG_MAX`, a chunk at a time.
fn read_output(cx: &Context, path: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut offset = 0u64;
    loop {
        let chunk = platform::output_read(cx, path, offset, LOG_CHUNK).ok()?;
        let n = chunk.len() as u64;
        bytes.extend_from_slice(&chunk);
        offset += n;
        if n < u64::from(LOG_CHUNK) || offset >= LOG_MAX {
            break;
        }
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn job_text(cx: &Context, job: &str, stream: &str) -> String {
    platform::output_read(cx, &format!(".jobs/{job}/{stream}.log"), 0, 256 << 10)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// What a build found: the log's problems plus biber's or BibTeX's, and
/// latexmk's own complaint when TeX never ran.
struct Found {
    parsed: log::Parsed,
    ran: bool,
}

fn read_build(cx: &Context, main: &str, job: &str) -> Found {
    let (root, _) = roots(cx);
    let (dir, _) = doc::split(main);
    let places = log::Places { dir, root: &root };
    let key = doc::key(main);
    let stem = doc::stem(main);
    let (mut parsed, ran) = match read_output(cx, &format!("{key}/{stem}.log")) {
        Some(text) if !text.is_empty() => (log::parse(&text, &places), true),
        _ => (log::Parsed::default(), false),
    };
    if let Some(blg) = read_output(cx, &format!("{key}/{stem}.blg")) {
        for (severity, message) in log::blg(&blg) {
            parsed.diags.push(log::Diag {
                file: main.to_string(),
                line: 1,
                severity,
                message: format!("bibliography: {message}"),
                context: None,
            });
        }
    }
    // A missing package: said as one, on the line that asks for it (TeX
    // points at the line after, where it stopped).
    for d in parsed.diags.iter_mut() {
        let Some(missing) = parsed
            .missing
            .iter()
            .find(|m| d.message.contains(m.as_str()))
        else {
            continue;
        };
        if let Some(n) = doc::missing_line(&head(cx, &d.file), missing) {
            d.line = n;
            d.context = None;
        }
        d.message = format!(
            "{} isn't installed ({missing} not found)",
            upper_first(&doc::missing_words(missing))
        );
    }
    if !ran {
        // latexmk or Perl complaining before TeX ran.
        let said = format!(
            "{}\n{}",
            job_text(cx, job, "stderr"),
            job_text(cx, job, "stdout")
        );
        if let Some(line) = said.lines().map(str::trim).find(|l| {
            !l.is_empty() && !l.starts_with("Rc files") && !l.starts_with("Latexmk: This is")
        }) {
            parsed.general.push(line.chars().take(300).collect());
        }
    }
    Found { parsed, ran }
}

fn upper_first(text: &str) -> String {
    let mut c = text.chars();
    c.next()
        .map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

fn diag_json(d: &log::Diag) -> Value {
    let mut v = json!({"file": d.file, "severity": d.severity, "line": d.line,
                       "message": d.message, "source": "latex"});
    if let Some(c) = &d.context {
        v["context"] = json!(c);
        if let Some((column, end)) = log::columns(c) {
            v["column"] = json!(column);
            v["end_line"] = json!(d.line);
            v["end_column"] = json!(end);
        }
    }
    v
}

fn has_pdf(cx: &Context, main: &str) -> bool {
    let want = format!("{}.pdf", doc::stem(main));
    platform::output_list(cx, &doc::key(main), 64)
        .map(|entries| entries.iter().any(|e| e.name == want))
        .unwrap_or(false)
}

/// Remember what the build read (the watch set, and a part's way to its
/// document), and drop a part's own results now that it belongs here.
fn record_inputs(cx: &Context, main: &str) {
    let key = doc::key(main);
    let (root, output) = roots(cx);
    let (dir, _) = doc::split(main);
    let Some(fls) = read_output(cx, &format!("{key}/{}.fls", doc::stem(main))) else {
        return;
    };
    let places = log::Places { dir, root: &root };
    let mut read = log::fls_inputs(&fls, &places, &output);
    read.truncate(INPUTS_KEPT);
    let parts: Vec<String> = read
        .iter()
        .filter(|p| p.as_str() != main)
        .cloned()
        .collect();
    let mut known = get(cx, "builds");
    for part in &parts {
        if known.remove(part).is_some() {
            let _ = platform::unpublish(cx, "diagnostics/1", &doc::key(part));
            let _ = platform::unpublish(cx, "output/1", &doc::key(part));
        }
    }
    save_builds(cx, known);
    let mut all = inputs(cx);
    for part in &parts {
        all.remove(part);
    }
    all.insert(main.to_string(), read);
    let kept: Vec<String> = get(cx, "builds").keys().cloned().collect();
    all.retain(|m, _| m == main || kept.contains(m));
    let mut watch: Vec<String> = all.values().flatten().cloned().collect();
    watch.sort();
    watch.dedup();
    watch.truncate(WATCH_MAX);
    let _ = platform::watch(cx, &watch);
    put(cx, "inputs", json!(all));
}

fn finish_build(cx: &Context, end: &JobEnd, main: &str) {
    let key = doc::key(main);
    let found = read_build(cx, main, &end.id);
    let parsed = &found.parsed;
    let items: Vec<Value> = parsed.diags.iter().map(diag_json).collect();
    let _ = platform::publish(cx, "diagnostics/1", &key, &json!({"items": items}));
    let from = platform::job_status(cx, &end.id)["from"]
        .as_str()
        .unwrap_or("path")
        .to_string();
    let errors = parsed.count("error");
    let warnings = parsed.count("warning");
    let state = if end.timed_out || (end.exit != Some(0) && errors == 0) {
        "failed"
    } else if end.exit == Some(0) {
        "ok"
    } else {
        "errors"
    };
    // What the user should act on, beside the problems list.
    let mut notice: Option<Value> = if end.timed_out {
        Some(json!({"tone": "bad", "title": "The build was stopped",
            "text": format!("It ran longer than its {} s limit: an endless loop, or a very large document (the limit is a setting).", wall(cx)),
            "log": true}))
    } else if state == "failed" {
        Some(json!({"tone": "bad", "title": "The build stopped",
            "text": parsed.general.first().cloned().unwrap_or_else(|| "latexmk stopped without saying why; the log has the details.".into()),
            "log": true}))
    } else {
        None
    };
    record_inputs(cx, main);
    let mut all = get(cx, "builds");
    let entry = all.entry(main.to_string()).or_insert_with(|| json!({}));
    let rerun = entry["pending"].as_bool() == Some(true);
    let by = entry["by"].as_str().unwrap_or("user").to_string();
    // A missing package: installed into the plugin's TinyTeX (once per
    // file), or explained for the host's own TeX Live.
    let tried: Vec<String> = entry["tried"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let next_missing = parsed.missing.iter().find(|m| !tried.contains(m)).cloned();
    let mut lookup: Option<String> = None;
    if let Some(file) = parsed.missing.first() {
        if from == "tool:tinytex" && setting_bool(cx, "install_missing_packages", true) {
            lookup = next_missing;
        } else if from == "path" {
            notice = Some(
                json!({"tone": "warn", "title": format!("{} isn't in this host's TeX Live", upper_first(&doc::missing_words(file))),
                "text": "Ask its admins for the package, load a fuller TeX Live in Environment settings, or turn on Use the plugin's TeX Live in this plugin's settings."}),
            );
        } else {
            notice = Some(
                json!({"tone": "warn", "title": format!("{} is missing", upper_first(&doc::missing_words(file))),
                "text": "Installing missing packages is off in this plugin's settings."}),
            );
        }
    }
    if let Some(o) = entry.as_object_mut() {
        o.remove("job");
        o.insert("pending".into(), json!(false));
        o.insert("state".into(), json!(state));
        o.insert("errors".into(), json!(errors));
        o.insert("warnings".into(), json!(warnings));
        o.insert("pages".into(), json!(parsed.pages));
        o.insert("ms".into(), json!(end.duration_ms));
        o.insert("ran".into(), json!(found.ran));
        o.insert("from".into(), json!(from));
        o.insert("last_job".into(), json!(end.id));
        match &notice {
            Some(n) => o.insert("notice".into(), n.clone()),
            None => o.remove("notice"),
        };
    }
    save_builds(cx, all);
    let label = match state {
        "ok" => doc::result_words(0, warnings, parsed.pages, end.duration_ms),
        "errors" => doc::result_words(errors, warnings, None, end.duration_ms),
        _ => "the build stopped".to_string(),
    };
    let _ = platform::publish(
        cx,
        "output/1",
        &key,
        &json!({"source": main, "output": doc::pdf(main), "state": state, "label": label,
                "finished_ms": host::now_ms(), "log": format!("output:{key}/{}.log", doc::stem(main))}),
    );
    platform::invalidate(cx, "document");
    if let Some(file) = lookup {
        look_up(cx, main, &file, &by);
    } else if rerun {
        let _ = build(cx, main, &by);
    }
}

fn set_status(cx: &Context, main: &str, update: impl FnOnce(&mut Obj)) {
    let mut all = get(cx, "builds");
    let entry = all.entry(main.to_string()).or_insert_with(|| json!({}));
    if let Some(o) = entry.as_object_mut() {
        update(o);
    }
    save_builds(cx, all);
    platform::invalidate(cx, "document");
}

/// Which package holds `file`: `tlmgr search --global --file` on the
/// plugin's TinyTeX (it reads the package list from its mirror).
fn look_up(cx: &Context, main: &str, file: &str, by: &str) {
    let f = file.to_string();
    set_status(cx, main, |o| {
        let mut tried: Vec<Value> = o
            .get("tried")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        tried.push(json!(f));
        o.insert("tried".into(), json!(tried));
        o.insert(
            "busy".into(),
            json!(format!("Looking up {}…", doc::missing_words(&f))),
        );
        o.remove("notice");
    });
    let spec = json!({"program": "tlmgr", "args": ["search", "--global", "--file", format!("/{file}")],
                      "prefer": "tool:tinytex", "label": format!("Looking up {file}"), "priority": by, "wall_s": 120});
    match platform::job_start(cx, &spec) {
        Ok(job) => remember_job(
            cx,
            &job,
            json!({"main": main, "kind": "search", "file": file}),
        ),
        Err(err) => set_status(cx, main, |o| {
            o.remove("busy");
            o.insert(
                "notice".into(),
                json!({"tone": "warn", "title": format!("{} is missing", upper_first(&doc::missing_words(file))), "text": format!("Looking it up failed: {err}")}),
            );
        }),
    }
}

/// Install `package` into the plugin's TinyTeX. `verified`: tlmgr checks
/// TeX Live's signature on the package list (it needs gpg on the host).
fn install_package(
    cx: &Context,
    main: &str,
    package: &str,
    verified: bool,
    by: &str,
) -> Result<(), String> {
    if !doc::valid_package(package) {
        return Err(format!("{package} is not a package name"));
    }
    let mut args = vec![json!("install")];
    if verified {
        args.push(json!("--verify-repo=main"));
    }
    args.push(json!(package));
    let spec = json!({"program": "tlmgr", "args": args, "prefer": "tool:tinytex",
                      "label": format!("Installing {package}"), "priority": by, "wall_s": 300});
    let job = platform::job_start(cx, &spec)?;
    remember_job(
        cx,
        &job,
        json!({"main": main, "kind": "install", "package": package, "verified": verified}),
    );
    let p = package.to_string();
    set_status(cx, main, |o| {
        o.insert("busy".into(), json!(format!("Installing {p}…")));
        o.remove("notice");
    });
    Ok(())
}

fn finish_search(cx: &Context, end: &JobEnd, main: &str, file: &str) {
    let out = job_text(cx, &end.id, "stdout");
    let by = get(cx, "builds")[main]["by"]
        .as_str()
        .unwrap_or("user")
        .to_string();
    match doc::package_for(&out, file) {
        Some(package) => {
            if let Err(err) = install_package(cx, main, &package, true, &by) {
                set_status(cx, main, |o| {
                    o.remove("busy");
                    o.insert(
                        "notice".into(),
                        json!({"tone": "warn", "title": format!("Couldn't install {package}"), "text": err}),
                    );
                });
            }
        }
        None => {
            let why = doc::tlmgr_words(&job_text(cx, &end.id, "stderr"));
            let f = file.to_string();
            set_status(cx, main, |o| {
                o.remove("busy");
                let text = if end.exit == Some(0) {
                    "No TeX Live package has it: check its name in the source.".to_string()
                } else {
                    format!("Looking it up failed: {why}.")
                };
                o.insert(
                    "notice".into(),
                    json!({"tone": "warn", "title": format!("{} is missing", upper_first(&doc::missing_words(&f))), "text": text}),
                );
            });
        }
    }
}

fn finish_install(cx: &Context, end: &JobEnd, main: &str, package: &str, verified: bool) {
    let by = get(cx, "builds")[main]["by"]
        .as_str()
        .unwrap_or("user")
        .to_string();
    let p = package.to_string();
    if end.exit == Some(0) {
        set_status(cx, main, |o| {
            o.remove("busy");
            o.remove("notice");
        });
        let _ = build(cx, main, &by);
        return;
    }
    let why = doc::tlmgr_words(&job_text(cx, &end.id, "stderr"));
    set_status(cx, main, |o| {
        o.remove("busy");
        // Without gpg the signature can't be checked: the user may still
        // install on a click, trusting the mirror's checksums alone.
        let mut n = json!({"tone": "warn", "title": format!("Couldn't install {p}"), "text": format!("{why}.")});
        if verified {
            n["offer"] = json!(p);
            n["text"] = json!(format!(
                "{why}. TeX Live's signature couldn't be checked here (it needs gpg)."
            ));
        }
        o.insert("notice".into(), n);
    });
}

fn finish(cx: &Context, end: &JobEnd) {
    let jobs = get(cx, "jobs");
    let Some(what) = jobs.get(&end.id).cloned() else {
        return;
    };
    let Some(main) = what["main"].as_str().map(str::to_string) else {
        return;
    };
    match what["kind"].as_str() {
        Some("build") => finish_build(cx, end, &main),
        Some("search") => finish_search(cx, end, &main, what["file"].as_str().unwrap_or("")),
        Some("install") => finish_install(
            cx,
            end,
            &main,
            what["package"].as_str().unwrap_or(""),
            what["verified"].as_bool() == Some(true),
        ),
        _ => {}
    }
}

fn open_now(cx: &Context, main: &str) -> bool {
    get(cx, "builds")
        .get(main)
        .and_then(|b| b["shown"].as_u64())
        .is_some_and(|t| host::now_ms().saturating_sub(t) < OPEN_MS)
}

fn document(cx: &Context, file: &str, narrow: bool) -> Value {
    let main = main_for(cx, file);
    let now = host::now_ms();
    let mut all = get(cx, "builds");
    let first_time = !all.contains_key(&main);
    {
        let entry = all.entry(main.clone()).or_insert_with(|| json!({}));
        entry["shown"] = json!(now);
        entry["touched"] = json!(now);
    }
    save_builds(cx, all);
    let pdf = has_pdf(cx, &main);
    let tinytex = platform::tool_state(cx, "tinytex");
    let installing = tinytex["installing"].as_bool() == Some(true);
    let missing = get(cx, "builds")
        .get(&main)
        .is_some_and(|b| b["missing"].is_string());
    if first_time && setting_bool(cx, "build_on_open", true) && !pdf {
        let _ = build(cx, &main, "user");
    } else if missing && !installing && !tinytex["installed"].is_null() {
        // TeX Live arrived (the Install button): build what waited for it.
        let _ = build(cx, &main, "user");
    }
    let b = get(cx, "builds")
        .get(&main)
        .cloned()
        .unwrap_or_else(|| json!({}));
    let busy = if b["job"].is_string() {
        Some("Building…".to_string())
    } else if installing {
        Some("Installing TeX Live…".to_string())
    } else {
        b["busy"].as_str().map(str::to_string)
    };
    let layouts = get(cx, "layouts");
    let layout = layouts
        .get(file)
        .and_then(Value::as_str)
        .unwrap_or("split")
        .to_string();
    let d = view::Doc {
        file,
        main: &main,
        layout: &layout,
        narrow,
        busy,
        state: b["state"].as_str().unwrap_or("new"),
        errors: b["errors"].as_u64().unwrap_or(0),
        warnings: b["warnings"].as_u64().unwrap_or(0),
        pages: b["pages"].as_u64(),
        ms: b["ms"].as_u64(),
        engine: b["engine"].as_str(),
        from: b["from"].as_str(),
        has_pdf: pdf,
        has_log: b["ran"].as_bool() == Some(true),
        no_engine: b["missing"].is_string(),
        notice: b.get("notice").filter(|n| n.is_object()),
        key: doc::key(&main),
        stem: doc::stem(&main),
        pdf: doc::pdf(&main),
        beside: doc::beside(&main),
    };
    view::tree(&d)
}

fn agent_main(cx: &Context, args: &Value) -> Result<String, String> {
    let path = args["path"]
        .as_str()
        .ok_or("compile_latex needs `path`: the .tex file to build (workspace-relative)")?
        .trim()
        .trim_start_matches("./");
    if !doc::is_tex(path) || path.starts_with('/') || path.split('/').any(|c| c == "..") {
        return Err(format!(
            "{path}: give a workspace-relative .tex file (the document, or one of its parts)"
        ));
    }
    host::stat(cx, path).map_err(|e| format!("{path}: {e}"))?;
    Ok(main_for(cx, path))
}

fn report(cx: &Context, main: &str, job: &str) -> ToolResult {
    let status = platform::job_status(cx, job);
    let found = read_build(cx, main, job);
    let p = &found.parsed;
    let exit = status["exit"].as_i64();
    let mut lines = Vec::new();
    if status["timed_out"].as_bool() == Some(true) {
        lines.push(format!("{main}: the build took longer than its time limit and was stopped (an endless loop, or a very large document)."));
    } else if exit == Some(0) {
        lines.push(format!(
            "{main} built{}: {} error(s), {} warning(s). The PDF is {} (the user sees it beside the source; Save PDF beside source copies it into the project).",
            p.pages.map_or(String::new(), |n| format!(" ({n} pages)")),
            p.count("error"),
            p.count("warning"),
            doc::pdf(main)
        ));
    } else {
        lines.push(format!(
            "{main} did not build cleanly: {} error(s), {} warning(s).",
            p.count("error"),
            p.count("warning")
        ));
    }
    for d in p.diags.iter().filter(|d| d.severity != "info").take(50) {
        let context = d
            .context
            .as_deref()
            .map_or(String::new(), |c| format!(" (at \"{c}\")"));
        lines.push(format!(
            "{}:{}: {}: {}{context}",
            d.file, d.line, d.severity, d.message
        ));
    }
    for m in &p.missing {
        lines.push(format!("missing: {m} (a package this TeX Live lacks; the plugin installs it into its own TinyTeX when that is what builds)"));
    }
    for g in p.general.iter().take(10) {
        lines.push(format!("error: {g}"));
    }
    let text = lines.join("\n");
    if exit == Some(0) {
        ToolResult::text(text)
    } else {
        ToolResult::error(text)
    }
}

const GUIDE: &str = "\
Writing LaTeX here (the LaTeX plugin is on in this workspace):

- For a new report, prefer Typst if its plugin is on; use LaTeX for a journal
  template, a thesis class, or an existing LaTeX project.
- Build with compile_latex {path}: latexmk runs every pass (bibliography
  included), waits, and returns each problem as file:line with the source text
  it stopped at. Fix and build again until it reports 0 errors.
- The engine: pdfLaTeX unless the file's head says `% !TEX program = xelatex`
  (or lualatex), or the workspace's setting does. Use xelatex or lualatex for
  system fonts (fontspec) or much Unicode.
- A part of a larger document (a chapter) can name its main file on its first
  line: `% !TEX root = ../thesis.tex`. Building a part builds its document.
- Bibliographies: biblatex with biber (`\\usepackage[backend=biber]{biblatex}`,
  `\\addbibresource{refs.bib}`), or natbib with BibTeX; latexmk runs whichever.
- A missing package (`File `x.sty' not found`) is installed automatically into
  the plugin's TinyTeX when that is what builds; on the host's own TeX Live,
  tell the user which package is missing.
- Never write the PDF or aux files into the project: the build output stays in
  the plugin's folder; the user saves a copy beside the source when they want.
- Shell escape stays as the site configured it (restricted); don't rely on
  --shell-escape (minted needs it: prefer listings).
";

impl Plugin for Latex {
    fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef::new(
                "compile_latex",
                "Build a LaTeX document (a workspace-relative .tex file, or one of its parts) to PDF on this host with latexmk, and return its errors and warnings as file:line lines with the source text at each error. Waits for the build (up to about 45 s); the user sees the PDF beside the source.",
                json!({"type": "object", "properties": {
                    "path": {"type": "string", "description": "The .tex file, workspace-relative"},
                }, "required": ["path"]}),
            ),
            ToolDef::new(
                "latex_guide",
                "How to write LaTeX in this workspace: building, engines, multi-file documents, bibliographies, missing packages.",
                json!({"type": "object", "properties": {}}),
            ),
        ]
    }

    fn instructions() -> Option<String> {
        Some("The LaTeX plugin is on here: build a .tex document with compile_latex {path} (it returns the errors to fix, with the source text at each) and read latex_guide before writing one.".into())
    }

    fn call_tool(cx: Context, name: &str, args: Value) -> ToolResult {
        match name {
            "latex_guide" => ToolResult::text(GUIDE),
            "compile_latex" => {
                let main = match agent_main(&cx, &args) {
                    Ok(m) => m,
                    Err(e) => return ToolResult::error(e),
                };
                match build(&cx, &main, "agent") {
                    Ok(job) => ToolResult::wait(
                        job.clone(),
                        format!("{main} is still building (job {job}); call compile_latex again for the result."),
                    ),
                    Err(err) if doc::not_found(&err) => ToolResult::error(format!(
                        "No TeX Live on this host ({err}). Ask the user to load theirs in Environment settings (on a cluster, often `module load texlive`) or to install TeX Live from the LaTeX plugin's card (one click)."
                    )),
                    Err(err) => ToolResult::error(err),
                }
            }
            other => ToolResult::error(format!("unknown tool {other}")),
        }
    }

    fn tool_resume(cx: Context, _name: &str, job: &str) -> ToolResult {
        let main = get(&cx, "jobs")
            .get(job)
            .and_then(|w| w["main"].as_str().map(str::to_string))
            .unwrap_or_else(|| "the document".into());
        report(&cx, &main, job)
    }

    fn render(cx: Context, view: &str, args: Value) -> Result<Value, String> {
        if view != "document" {
            return Err(format!("no view {view}"));
        }
        let file = args["file"]
            .as_str()
            .ok_or("the LaTeX view opens a .tex file")?;
        Ok(document(
            &cx,
            file,
            args["width"].as_str() == Some("narrow"),
        ))
    }

    fn on_action(
        cx: Context,
        view: &str,
        action: &str,
        payload: Value,
    ) -> Result<Option<Value>, String> {
        if action == "layout" {
            let file = payload["file"].as_str().ok_or("which file?")?;
            let mode = match payload["value"].as_str() {
                Some(m @ ("split" | "source" | "pdf")) => m,
                _ => return Err("a layout is split, source or pdf".into()),
            };
            let mut layouts = get(&cx, "layouts");
            layouts.insert(file.to_string(), json!(mode));
            while layouts.len() > 64 {
                let first = layouts.keys().next().cloned();
                if let Some(k) = first {
                    layouts.remove(&k);
                }
            }
            put(&cx, "layouts", Value::Object(layouts));
            return Ok(Some(document(&cx, file, false)));
        }
        let main = payload["main"].as_str().ok_or("which document?")?;
        match action {
            "build" => match build(&cx, main, "user") {
                Ok(_) => Ok(None),
                Err(err) if doc::not_found(&err) => Ok(None),
                Err(err) => Err(err),
            },
            // The user's click after a signed install failed: the mirror's
            // checksums alone.
            "install-package" => {
                let package = payload["package"].as_str().ok_or("which package?")?;
                install_package(&cx, main, package, false, "user").map(|()| None)
            }
            other => Err(format!("{view} has no action {other}")),
        }
    }

    fn on_event(cx: Context, event: Event) -> Option<String> {
        match event {
            Event::FileSaved(path) => {
                let main = main_for(&cx, &path);
                if setting_bool(&cx, "build_on_save", true) && doc::is_tex(&main) {
                    let _ = build(&cx, &main, "user");
                }
            }
            Event::FileChanged(path) => {
                let main = main_for(&cx, &path);
                if doc::is_tex(&main)
                    && setting_bool(&cx, "build_on_agent_edit", true)
                    && open_now(&cx, &main)
                {
                    let _ = build(&cx, &main, "agent");
                }
            }
            Event::JobFinished(end) => finish(&cx, &end),
            Event::SettingsChanged(_) => platform::invalidate(&cx, "document"),
            _ => {}
        }
        None
    }
}

chimaera_plugin_api::export!(Latex);
