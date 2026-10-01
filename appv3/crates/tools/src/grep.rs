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
    Linear { line: regex::Regex, prefilter: Option<regex::bytes::Regex> },
    Fancy(fancy_regex::Regex),
}

impl Matcher {
    fn is_match(&self, text: &str) -> bool {
        match self {
            Matcher::Linear { line, .. } => line.is_match(text),
            Matcher::Fancy(rx) => rx.is_match(text).unwrap_or(false),
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
        return Ok(Matcher::Linear { line, prefilter });
    }
    fancy_regex::Regex::new(pattern).map(Matcher::Fancy).map_err(|e| ToolError::Execution(format!("Invalid regex: {e}")))
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
                    || (has_nl && {
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

/// Candidate files in walk order: v2's `os.walk` (a directory's files, then
/// its subdirectories, depth first), with names sorted so the order — and
/// which hits survive `max_results` — is the same on every OS/filesystem.
fn candidate_files(root: &Path, include: &regex::Regex, gi: &Gitignore, denied: &DeniedPaths, deadline: Instant) -> Result<Vec<PathBuf>, ToolError> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if Instant::now() > deadline {
            return Err(timeout_error());
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
                out.push(fp);
            }
        }
        let mut kept: Vec<String> = dirs.into_iter().filter(|d| !NOISE_DIR_NAMES.contains(&d.as_str()) && !is_gitignored(gi, &join(d), true)).collect();
        kept.reverse();
        stack.extend(kept.into_iter().map(|d| dir.join(d)));
    }
    Ok(out)
}

fn timeout_error() -> ToolError {
    ToolError::Execution(format!("grep_files scan timed out after {SCAN_TIMEOUT_S}s — pattern may be too complex or directory too large"))
}

/// Scan `files` in parallel, keeping walk order, stopping at `max` hits.
fn scan_ordered(m: &Matcher, files: &[PathBuf], denied: &DeniedPaths, max: usize, deadline: Instant) -> Result<Vec<String>, ToolError> {
    use rayon::prelude::*;
    let chunk = (rayon::current_num_threads() * 16).max(16);
    let mut hits = vec![];
    for batch in files.chunks(chunk) {
        if Instant::now() > deadline {
            return Err(timeout_error());
        }
        let per_file: Vec<Vec<String>> = batch.par_iter().map(|fp| scan_file(m, fp, &denied.display_path(fp), max)).collect();
        for line in per_file.into_iter().flatten() {
            hits.push(line);
            if hits.len() >= max {
                return Ok(hits);
            }
        }
    }
    Ok(hits)
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
        let task = tokio::task::spawn_blocking(move || -> Result<Vec<String>, ToolError> {
            let m = compile_pattern(&pattern)?;
            if resolved.is_file() {
                return Ok(scan_file(&m, &resolved, &denied.display_path(&resolved), max_results));
            }
            let deadline = Instant::now() + std::time::Duration::from_secs(SCAN_TIMEOUT_S);
            let gi = load_gitignore(&resolved);
            let inc = regex::Regex::new(&fnmatch_translate(&include)).map_err(ToolError::exec)?;
            let files = candidate_files(&resolved, &inc, &gi, &denied, deadline)?;
            scan_ordered(&m, &files, &denied, max_results, deadline)
        });
        let hits = match tokio::time::timeout(std::time::Duration::from_secs(SCAN_TIMEOUT_S), task).await {
            Ok(r) => r.map_err(ToolError::exec)??,
            Err(_) => return Err(timeout_error()),
        };
        if hits.is_empty() {
            return Ok(ToolOutput::Text(no_match));
        }
        Ok(ToolOutput::Text(hits.join("\n")))
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
}
