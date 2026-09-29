# chimaera-plugin-latex

LaTeX for [Chimaera](https://github.com/martinappberg/chimaera): `.tex` files open as
source beside their PDF, saving builds with latexmk, errors become marks in the
editor, and agents check their own work with `compile_latex`.

A Chimaera workbench plugin (plugin API 0.2): a Rust crate compiled to one portable
WebAssembly component (`plugin.wasm`) plus its manifest (`plugin.toml`). It is
**privileged**: it runs `latexmk` (which runs the engines and biber or BibTeX) and
`tlmgr` as jobs the Chimaera daemon starts under its limits, and it can download
TinyTeX into its own folder when you click Install. The component itself cannot open
files outside the workspace, run anything, or reach the network; everything goes
through the daemon.

## What it does

- **The document view.** A `.tex` file opens as a split: the editor on the left, the
  PDF on the right, the problems below it. **Text** is one click away.
- **Builds** with `latexmk` (every pass, the bibliography included) on open (when
  there is no build yet), on save, and when an agent edits a file of an open
  document. One build per document at a time; a change during a build queues one
  rebuild.
- **The engine**: pdfLaTeX, unless the file's head says `% !TEX program = xelatex` (or
  `lualatex`) or the workspace's setting does.
- **Errors** from the log (`-file-line-error`, the input-file stack, package and class
  warnings, boxes, biber's `.blg`) become editor marks on the exact control sequence
  TeX stopped at, and a problems list with **Go to** and **Ask agent**.
- **Multi-file documents**: a part names its document with `% !TEX root =
  ../thesis.tex`, or is found in the last build's recorder file; saving a chapter
  builds the thesis.
- **Your TeX Live first**: whatever your terminals find (a `module load texlive` in
  Environment settings counts). Where there is none, **Install TeX Live (TinyTeX,
  about 150 MB)** downloads the pinned TinyTeX release, checks its sha256 and keeps it
  in the plugin's own folder: no PATH or shell-file changes.
- **Missing packages**: a build on the plugin's TinyTeX that needs a package it lacks
  looks it up and installs it with `tlmgr` (TeX Live's signature checked where the
  host has `gpg`; otherwise an **Install** button), then builds again. On your own TeX
  Live it never changes anything; it says which package is missing.
- **Safe on a shared host**: `latexmk -norc` always (a project's `latexmkrc` is Perl
  and never runs), the site's restricted shell escape unchanged, and the daemon's job
  limits (time, memory, priority, output caps). Build output stays in the plugin's
  folder; **Save PDF beside source** copies the PDF next to the `.tex` on your click.

## For agents

- `compile_latex {path}` builds the document, waits, and returns each problem as
  `file:line: severity: message (at "source text")`.
- `latex_guide` explains how to write LaTeX here.

## Settings

Engine (pdfLaTeX), build on save / on open / when an agent edits (on), use the
plugin's TeX Live (off), install missing packages (on), time limit (180 s).

## Develop

```sh
cargo test                                            # the parser and rules, natively
cargo build --release --target wasm32-wasip2          # the component
chimaera plugin add --path <dir with plugin.wasm and plugin.toml>
```

`src/log.rs` (the log parser; its fixtures in `tests/logs/` are real TeX Live 2026
logs), `src/doc.rs` (main files, engines, build folders, tlmgr output) are plain Rust
with native tests; `src/lib.rs` is the glue to the host. See Chimaera's
[plugin authoring guide](https://github.com/martinappberg/chimaera/blob/main/docs/agent-guides/plugins.md).

## Release

Bump `version` in both `Cargo.toml` and `plugin.toml`, commit, and push a tag
`vX.Y.Z`. The release workflow publishes `plugin.wasm`, `plugin.toml` and
`SHA256SUMS`. A new TinyTeX month is a new plugin release: update the
`[[tools.artifacts]]` (URL, size, and the sha256 taken from each file).
