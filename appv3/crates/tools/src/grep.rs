//! `grep` — port of `filesystem/grep.py` (+ `_ignore.py`).

use crate::args::Args;
use crate::denied::fnmatch_translate;
use crate::denied::DeniedPaths;
use crate::{Tool, ToolContext, ToolError, ToolOutput, ToolResult};
use async_trait::async_trait;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const NOISE_DIR_NAMES: &[&str] = &[
    ".git",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".ruff_cache",
    ".pytest_cache",
    ".tox",
    ".nox",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".gradle",
    ".terraform",
];
const MAX_PATTERN_LEN: usize = 500;
const SCAN_TIMEOUT_S: u64 = 10;
const BINARY_SNIFF_BYTES: usize = 4096;
/// Files are read this much at a time (rounded to whole lines), so a large
/// log costs ~1 MB per scanning thread instead of several times its size.
const SCAN_CHUNK_BYTES: usize = 1 << 20;

/// Root `.gitignore` only (v2 limitation preserved).
pub fn load_gitignore(root: &Path) -> Gitignore {
    let mut b = GitignoreBuilder::new(root);
    let gi = root.join(".gitignore");
    if gi.is_file() {
        let _ = b.add(gi);
    }
    b.build().unwrap_or_else(|_| Gitignore::empty())
}

pub fn is_gitignored(gi: &Gitignore, rel: &str, is_dir: bool) -> bool {
    gi.matched_path_or_any_parents(rel, is_dir).is_ignore()
}

/// A compiled grep pattern with Python `re.search` semantics per line.
///
/// Patterns the linear-time `regex` crate accepts run on it (no
/// catastrophic backtracking, whole-file prefilter). Patterns that need
/// lookaround or backreferences fall back to `fancy_regex`, whose
/// backtracking is bounded by its step limit.
pub enum Matcher {
    /// `needs_nl`: the pattern can match the line's own `\n` (see
    /// [`can_use_trailing_newline`]), so a line that fails is tried again
    /// with it.
    Linear {
        line: regex::Regex,
        prefilter: Option<regex::bytes::Regex>,
        needs_nl: bool,
    },
    Fancy(fancy_regex::Regex),
}

impl Matcher {
    fn is_match(&self, text: &str) -> bool {
        match self {
            Matcher::Linear { line, .. } => line.is_match(text),
            Matcher::Fancy(rx) => rx.is_match(text).unwrap_or(false),
        }
    }

    fn needs_trailing_newline(&self) -> bool {
        match self {
            Matcher::Linear { needs_nl, .. } => *needs_nl,
            Matcher::Fancy(_) => true,
        }
    }

    /// Cheap whole-file check; `false` only if no line can match.
    fn may_match_file(&self, data: &[u8]) -> bool {
        match self {
            Matcher::Linear { prefilter: Some(pf), .. } => std::str::from_utf8(data).is_err() || pf.is_match(data),
            _ => true,
        }
    }
}

pub fn compile_pattern(pattern: &str) -> Result<Matcher, ToolError> {
    if pattern.chars().count() > MAX_PATTERN_LEN {
        return Err(ToolError::Execution(format!("Pattern too long ({} chars, max {MAX_PATTERN_LEN})", pattern.chars().count())));
    }
    if let Ok(line) = regex::Regex::new(pattern) {
        // Multi-line form over the whole (newline-normalised) file: every
        // per-line match is also a match here, except for anchors that mean
        // "start/end of text", which then skip the prefilter.
        let anchored = ["\\A", "\\z", "\\Z"].iter().any(|a| pattern.contains(a));
        let prefilter = if anchored { None } else { regex::bytes::Regex::new(&format!("(?m:{pattern})")).ok() };
        return Ok(Matcher::Linear { line, prefilter, needs_nl: can_use_trailing_newline(pattern) });
    }
    fancy_regex::Regex::new(pattern).map(Matcher::Fancy).map_err(|e| ToolError::Execution(format!("Invalid regex: {e}")))
}

/// Whether matching `line + "\n"` can succeed where `line` alone fails:
/// only if the pattern can match a `\n` itself (a literal, `\s`, a negated
/// class, `(?s).`) or uses multi-line `^`/`$`, which can match around it.
/// Other assertions see `\n` and end-of-text alike. Unparsable: assume yes.
fn can_use_trailing_newline(pattern: &str) -> bool {
    use regex_syntax::hir::{Class, Hir, HirKind, Look};
    fn walk(h: &Hir) -> bool {
        match h.kind() {
            HirKind::Empty => false,
            HirKind::Literal(l) => l.0.contains(&b'\n'),
            HirKind::Class(Class::Unicode(c)) => c.ranges().iter().any(|r| r.start() <= '\n' && '\n' <= r.end()),
            HirKind::Class(Class::Bytes(c)) => c.ranges().iter().any(|r| r.start() <= b'\n' && b'\n' <= r.end()),
            HirKind::Look(l) => matches!(l, Look::StartLF | Look::EndLF | Look::StartCRLF | Look::EndCRLF),
            HirKind::Repetition(r) => walk(&r.sub),
            HirKind::Capture(c) => walk(&c.sub),
            HirKind::Concat(hs) | HirKind::Alternation(hs) => hs.iter().any(walk),
        }
    }
    regex_syntax::parse(pattern).map(|h| walk(&h)).unwrap_or(true)
}

/// Python universal newlines: `\r\n` and lone `\r` become `\n`.
fn normalise_newlines(data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if memchr::memchr(b'\r', data).is_none() {
        return std::borrow::Cow::Borrowed(data);
    }
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if b == b'\r' {
            out.push(b'\n');
            if data.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
        } else {
            out.push(b);
        }
        i += 1;
    }
    std::borrow::Cow::Owned(out)
}

/// Search one file like v2 (`TextIOWrapper(errors="replace")` +
/// `compiled.search(line)`, where `line` keeps its `\n`). Returns hits in
/// line order, at most `max`.
fn scan_file(m: &Matcher, fpath: &Path, display: &str, max: usize) -> Vec<String> {
    match std::fs::File::open(fpath) {
        Ok(f) => scan_reader(m, f, display, max, SCAN_CHUNK_BYTES),
        Err(_) => vec![],
    }
}

/// Where a chunk can end: after its last `\n`, or after a lone `\r` (one
/// not at the very end, which may be the first half of `\r\n`). At EOF,
/// everything left.
fn chunk_cut(buf: &[u8], eof: bool) -> Option<usize> {
    if eof {
        return Some(buf.len());
    }
    if let Some(i) = memchr::memrchr(b'\n', buf) {
        return Some(i + 1);
    }
    memchr::memrchr(b'\r', &buf[..buf.len().saturating_sub(1)]).map(|i| i + 1)
}

/// [`scan_file`] over any reader, `chunk` bytes at a time. Chunks hold whole
/// lines, so the per-chunk prefilter and the line numbers match a scan of
/// the whole file; reading stops at `max` hits.
fn scan_reader(m: &Matcher, mut r: impl Read, display: &str, max: usize, chunk: usize) -> Vec<String> {
    let mut hits = vec![];
    let mut buf: Vec<u8> = Vec::new();
    let mut line_no = 0usize;
    let mut first = true;
    let mut with_nl = String::new();
    loop {
        let want = if first { chunk.max(BINARY_SNIFF_BYTES) } else { chunk };
        let n = match r.by_ref().take(want as u64).read_to_end(&mut buf) {
            Ok(n) => n,
            Err(_) => return hits,
        };
        if first {
            if buf[..buf.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
                return hits;
            }
            first = false;
        }
        let eof = n == 0;
        let Some(cut) = chunk_cut(&buf, eof) else { continue };
        let region = normalise_newlines(&buf[..cut]);
        if !m.may_match_file(&region) {
            line_no += memchr::memchr_iter(b'\n', &region).count() + usize::from(eof && !region.is_empty() && !region.ends_with(b"\n"));
        } else {
            let text = String::from_utf8_lossy(&region);
            let ends_with_nl = text.ends_with('\n');
            let mut lines: Vec<&str> = text.split('\n').collect();
            if ends_with_nl || text.is_empty() {
                lines.pop();
            }
            let last = lines.len().saturating_sub(1);
            for (i, line) in lines.iter().enumerate() {
                line_no += 1;
                let has_nl = i < last || ends_with_nl;
                // `$` in Python also matches before the final `\n`; patterns
                // that consume the newline (`\s$`, `\n`) need the line with it.
                let hit = m.is_match(line)
                    || (has_nl && m.needs_trailing_newline() && {
                        with_nl.clear();
                        with_nl.push_str(line);
                        with_nl.push('\n');
                        m.is_match(&with_nl)
                    });
                if hit {
                    let shown: String = line.trim_end().chars().take(200).collect();
                    hits.push(format!("{display}:{line_no}: {shown}"));
                    if hits.len() >= max {
                        return hits;
                    }
                }
            }
        }
        drop(region);
        buf.drain(..cut);
        if eof {
            return hits;
        }
    }
}

/// Search the tree under `root` in walk order: v2's `os.walk` (a directory's
/// files, then its subdirectories, depth first), with names sorted so the
/// order — and which hits survive `max` — is the same on every OS. Files are
/// scanned in parallel `batch`es as the walk finds them, so a search with
/// early hits stops without walking the rest. Returns `(hits, timed_out)`;
/// on time-out the hits found so far are kept.
#[allow(clippy::too_many_arguments)]
fn search_tree(
    root: &Path,
    include: &regex::Regex,
    gi: &Gitignore,
    denied: &DeniedPaths,
    m: &Matcher,
    max: usize,
    batch: usize,
    expired: &dyn Fn() -> bool,
) -> (Vec<String>, bool) {
    let mut hits = vec![];
    let mut pending: Vec<PathBuf> = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if expired() {
            return (hits, true);
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut dirs = vec![];
        let mut files = vec![];
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            // os.walk follows neither dir symlinks; file symlinks count as files
            let name = e.file_name().to_string_lossy().to_string();
            if ft.is_dir() {
                dirs.push(name);
            } else if ft.is_file() || (ft.is_symlink() && e.path().is_file()) {
                files.push(name);
            }
        }
        files.sort();
        dirs.sort();
        let rel_dir = dir.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        let join = |n: &str| if rel_dir.is_empty() { n.to_string() } else { format!("{rel_dir}/{n}") };
        for f in files {
            if !include.is_match(&f) || is_gitignored(gi, &join(&f), false) {
                continue;
            }
            let fp = dir.join(&f);
            if !denied.is_denied_read_path(&fp) {
                pending.push(fp);
            }
        }
        if pending.len() >= batch {
            if scan_batch(m, &pending, denied, max, &mut hits) {
                return (hits, false);
            }
            pending.clear();
            if expired() {
                return (hits, true);
            }
        }
        let mut kept: Vec<String> = dirs.into_iter().filter(|d| !NOISE_DIR_NAMES.contains(&d.as_str()) && !is_gitignored(gi, &join(d), true)).collect();
        kept.reverse();
        stack.extend(kept.into_iter().map(|d| dir.join(d)));
    }
    scan_batch(m, &pending, denied, max, &mut hits);
    (hits, false)
}

fn timeout_error() -> ToolError {
    ToolError::Execution(format!("grep_files scan timed out after {SCAN_TIMEOUT_S}s — pattern may be too complex or directory too large"))
}

/// Scan `files` in parallel into `hits`, keeping their order. True once
/// `max` hits are in.
fn scan_batch(m: &Matcher, files: &[PathBuf], denied: &DeniedPaths, max: usize, hits: &mut Vec<String>) -> bool {
    use rayon::prelude::*;
    let left = max - hits.len();
    let per_file: Vec<Vec<String>> = files.par_iter().map(|fp| scan_file(m, fp, &denied.display_path(fp), left)).collect();
    for line in per_file.into_iter().flatten() {
        hits.push(line);
        if hits.len() >= max {
            return true;
        }
    }
    false
}

/// The tool's text for `hits`, noting when the search stopped early.
fn render_hits(hits: Vec<String>, timed_out: bool) -> String {
    let mut out = hits.join("\n");
    if timed_out {
        out.push_str(&format!("\n\n[grep stopped after {SCAN_TIMEOUT_S}s: these results are partial. Narrow the directory or include pattern for a complete search.]"));
    }
    out
}

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    async fn run(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        let mut a = Args::new("grep", &args);
        let pattern = a.req_str(&["pattern", "query", "regex"]);
        let directory = a.str_or(&["directory", "dir", "path"], ".");
        let include = a.str_or(&["include", "glob", "file_pattern"], "*");
        let max_results = a.opt_int(&["max_results"], Some(1), None).unwrap_or(100) as usize;
        if let Err(ToolError::Execution(e)) = compile_pattern(&pattern) {
            a.err("pattern", &format!("Value error, {e}"));
        }
        a.finish()?;
        let denied = ctx.denied.clone();
        let resolved = denied.validate_read_path(&directory)?;
        if !resolved.exists() {
            return Err(ToolError::Execution(format!("File or directory not found: {}", denied.display_path(&resolved))));
        }
        let no_match = format!("No matches for pattern '{pattern}' in {} (include={include})", denied.display_path(&resolved));
        let task = tokio::task::spawn_blocking(move || -> Result<(Vec<String>, bool), ToolError> {
            let m = compile_pattern(&pattern)?;
            if resolved.is_file() {
                return Ok((scan_file(&m, &resolved, &denied.display_path(&resolved), max_results), false));
            }
            let deadline = Instant::now() + std::time::Duration::from_secs(SCAN_TIMEOUT_S);
            let gi = load_gitignore(&resolved);
            let inc = regex::Regex::new(&fnmatch_translate(&include)).map_err(ToolError::exec)?;
            let batch = (rayon::current_num_threads() * 16).max(16);
            Ok(search_tree(&resolved, &inc, &gi, &denied, &m, max_results, batch, &|| Instant::now() > deadline))
        });
        // The walk keeps its own deadline and returns partial hits; this is
        // only a backstop for a single file that will not finish.
        let (hits, timed_out) = match tokio::time::timeout(std::time::Duration::from_secs(SCAN_TIMEOUT_S + 5), task).await {
            Ok(r) => r.map_err(ToolError::exec)??,
            Err(_) => return Err(timeout_error()),
        };
        if hits.is_empty() {
            if timed_out {
                return Err(timeout_error());
            }
            return Ok(ToolOutput::Text(no_match));
        }
        Ok(ToolOutput::Text(render_hits(hits, timed_out)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn ctx(ws: &Path) -> ToolContext {
        ToolContext {
            session_id: None,
            agent_name: "t".into(),
            tool_call_id: "c".into(),
            denied: Arc::new(DeniedPaths::with(ws, None, Some(vec![]), Some(vec![]))),
            workspace: None,
            output: None,
            metadata: Default::default(),
            messages: None,
        }
    }

    async fn grep(ws: &Path, args: Value) -> String {
        match GrepTool.run(&ctx(ws), args).await.unwrap() {
            ToolOutput::Text(t) => t,
            o => panic!("{o:?}"),
        }
    }

    fn lines_of(out: &str) -> Vec<String> {
        // "path:line: text" with the workspace prefix stripped to the file name
        out.lines().map(|l| l.rsplit(['/', '\\']).next().unwrap_or(l).to_string()).collect()
    }

    #[tokio::test]
    async fn python_line_semantics_including_crlf() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("crlf.txt"), b"foo\r\nbar\r\nold\rmac\n").unwrap();
        std::fs::write(d.path().join("lf.txt"), b"x foo \nlast foo").unwrap();
        // `$` before a CRLF / lone CR, like Python universal newlines.
        let out = grep(d.path(), serde_json::json!({"pattern": "foo$|^mac$"})).await;
        assert_eq!(lines_of(&out), vec!["crlf.txt:1: foo", "crlf.txt:4: mac", "lf.txt:2: last foo"], "{out}");
        // `\s$` can consume the line's own newline (v2 searches `line` with `\n`).
        let out = grep(d.path(), serde_json::json!({"pattern": "foo\\s$", "include": "lf.txt"})).await;
        assert_eq!(lines_of(&out), vec!["lf.txt:1: x foo"], "{out}");
        // `^` anchors per line (prefilter must not drop line 2).
        let out = grep(d.path(), serde_json::json!({"pattern": "^bar", "include": "crlf.txt"})).await;
        assert_eq!(lines_of(&out), vec!["crlf.txt:2: bar"], "{out}");
    }

    #[tokio::test]
    async fn fancy_fallback_and_linear_time() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "foobar\nfoobaz\n").unwrap();
        let out = grep(d.path(), serde_json::json!({"pattern": "foo(?=baz)"})).await;
        assert_eq!(lines_of(&out), vec!["a.txt:2: foobaz"], "{out}");
        assert!(matches!(compile_pattern("foo(?=baz)").unwrap(), Matcher::Fancy(_)));
        assert!(matches!(compile_pattern("fo+").unwrap(), Matcher::Linear { .. }));
        // Catastrophic for a backtracking engine; linear here.
        std::fs::write(d.path().join("evil.txt"), format!("{}b\n", "a".repeat(5000))).unwrap();
        let t = Instant::now();
        let out = grep(d.path(), serde_json::json!({"pattern": "(a+)+$", "include": "evil.txt"})).await;
        assert!(out.starts_with("No matches"), "{out}");
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
        let err = GrepTool.run(&ctx(d.path()), serde_json::json!({"pattern": "(unclosed"})).await.unwrap_err();
        assert!(format!("{err:?}").contains("Invalid regex"), "{err:?}");
    }

    /// Files are scanned in chunks of whole lines; the hits, line numbers
    /// and newline handling must not depend on where a chunk ends.
    #[test]
    fn chunked_scan_matches_a_whole_file_scan() {
        let contents: [&[u8]; 6] = [
            b"foo\r\nbar\r\nold\rmac\nx foo \nlast foo",
            b"a\nfoo\n\nfoo bar\nTi\xe1\xba\xbfng foo Vi\xe1\xbb\x87t\n",
            b"\r\r\nfoo\r",
            b"no trailing newline foo",
            b"foo\n",
            b"\xff\xfe broken utf8 foo\nfoo\n",
        ];
        for pattern in ["foo$", "foo\\s$", "^foo", "foo", "^$", "t$"] {
            let m = compile_pattern(pattern).unwrap();
            for data in contents {
                let whole = scan_reader(&m, data, "f", 100, usize::MAX);
                for chunk in [1, 2, 3, 5, 8, 64] {
                    assert_eq!(scan_reader(&m, data, "f", 100, chunk), whole, "pattern {pattern:?} chunk {chunk} data {:?}", String::from_utf8_lossy(data));
                }
                assert_eq!(scan_reader(&m, data, "f", 1, 2), whole.into_iter().take(1).collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn newline_check_is_kept_only_for_patterns_that_can_use_it() {
        for p in ["foo", "foo$", "^foo", "^$", "a.b", "\\bfoo\\b", "[a-z]+", "(?i)todo|fixme", "x\\z"] {
            assert!(!can_use_trailing_newline(p), "{p}");
        }
        for p in ["foo\\s$", "[^a]", "\\n", "(?m)^$", "(?m)foo$", "(?s)a.b", "\\W", "\\D", "a[\\s,]b", "(unclosed"] {
            assert!(can_use_trailing_newline(p), "{p}");
        }
    }

    #[test]
    fn skipping_the_newline_check_never_changes_hits() {
        let data: &[u8] = b"foo\r\nbar \nold\rmac\nx foo \nTi\xe1\xba\xbfng\n\nlast foo";
        for pattern in ["foo", "foo$", "^foo", "^$", "a.b", "\\bfoo\\b", "t$", "ac$", "^\\w+$"] {
            let fast = compile_pattern(pattern).unwrap();
            assert!(matches!(fast, Matcher::Linear { needs_nl: false, .. }), "{pattern}");
            let Matcher::Linear { line, prefilter, .. } = compile_pattern(pattern).unwrap() else { unreachable!() };
            let always = Matcher::Linear { line, prefilter, needs_nl: true };
            assert_eq!(scan_reader(&fast, data, "f", 100, 7), scan_reader(&always, data, "f", 100, 7), "{pattern}");
        }
    }

    #[tokio::test]
    async fn deterministic_walk_order_and_limit() {
        let d = tempfile::tempdir().unwrap();
        for f in ["b.txt", "a.txt", "sub/z.txt", "sub/deeper/y.txt", "c.txt", "node_modules/n.txt"] {
            let p = d.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "hit\n").unwrap();
        }
        std::fs::write(d.path().join(".gitignore"), "c.txt\n").unwrap();
        let out = grep(d.path(), serde_json::json!({"pattern": "hit"})).await;
        // A directory's files first (sorted), then its subdirectories; noise
        // dirs and gitignored files skipped.
        assert_eq!(lines_of(&out), vec!["a.txt:1: hit", "b.txt:1: hit", "z.txt:1: hit", "y.txt:1: hit"], "{out}");
        let out = grep(d.path(), serde_json::json!({"pattern": "hit", "max_results": 3})).await;
        assert_eq!(lines_of(&out), vec!["a.txt:1: hit", "b.txt:1: hit", "z.txt:1: hit"], "{out}");
    }

    fn hit_tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for f in ["a.txt", "b.txt", "sub/c.txt", "sub/d.txt", "sub/deeper/e.txt", "z/f.txt"] {
            let p = d.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "hit\n").unwrap();
        }
        d
    }

    fn search(root: &Path, max: usize, batch: usize, expired: &dyn Fn() -> bool) -> (Vec<String>, bool) {
        let denied = DeniedPaths::with(root, None, Some(vec![]), Some(vec![]));
        let inc = regex::Regex::new(&fnmatch_translate("*")).unwrap();
        search_tree(root, &inc, &load_gitignore(root), &denied, &compile_pattern("hit").unwrap(), max, batch, expired)
    }

    #[test]
    fn search_keeps_walk_order_for_any_batch_size() {
        let d = hit_tree();
        let (all, timed_out) = search(d.path(), 100, 1000, &|| false);
        assert!(!timed_out);
        assert_eq!(lines_of(&all.join("\n")), ["a.txt:1: hit", "b.txt:1: hit", "c.txt:1: hit", "d.txt:1: hit", "e.txt:1: hit", "f.txt:1: hit"]);
        for batch in [1, 2, 3] {
            assert_eq!(search(d.path(), 100, batch, &|| false).0, all, "batch {batch}");
            assert_eq!(search(d.path(), 4, batch, &|| false).0, all[..4], "batch {batch}");
        }
    }

    #[test]
    fn a_timed_out_search_keeps_what_it_found() {
        let d = hit_tree();
        let (all, _) = search(d.path(), 100, 1000, &|| false);
        let calls = std::cell::Cell::new(0);
        let (partial, timed_out) = search(d.path(), 100, 1, &|| {
            calls.set(calls.get() + 1);
            calls.get() > 3
        });
        assert!(timed_out);
        assert!(!partial.is_empty() && partial.len() < all.len(), "{partial:?}");
        assert_eq!(partial, all[..partial.len()]);
    }

    #[test]
    fn partial_results_say_so() {
        assert_eq!(render_hits(vec!["a:1: x".into()], false), "a:1: x");
        let out = render_hits(vec!["a:1: x".into(), "b:2: y".into()], true);
        assert!(out.starts_with("a:1: x\nb:2: y\n\n") && out.contains("stopped after 10s"), "{out}");
    }
}
