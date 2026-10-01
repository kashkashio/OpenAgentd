//! `glob` — port of `filesystem/glob.py`.

use crate::args::Args;
use crate::grep::{is_gitignored, load_gitignore, NOISE_DIR_NAMES};
use crate::{Tool, ToolContext, ToolError, ToolOutput, ToolResult};
use async_trait::async_trait;
use globset::{GlobBuilder, GlobMatcher};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MAX_BRACE_VARIANTS: usize = 64;
const GLOB_TIMEOUT_S: u64 = 10;

pub fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(start) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let bytes: Vec<char> = pattern.chars().collect();
    let cstart = pattern[..start].chars().count();
    let mut depth = 0;
    let mut end = None;
    for (i, c) in bytes.iter().enumerate().skip(cstart) {
        if *c == '{' {
            depth += 1;
        } else if *c == '}' {
            depth -= 1;
            if depth == 0 {
                end = Some(i);
                break;
            }
        }
    }
    let Some(end) = end else {
        return vec![pattern.to_string()];
    };
    let prefix: String = bytes[..cstart].iter().collect();
    let body: Vec<char> = bytes[cstart + 1..end].to_vec();
    let suffix: String = bytes[end + 1..].iter().collect();
    let mut options = vec![];
    let mut cur = String::new();
    let mut d = 0;
    for c in body {
        if c == '{' {
            d += 1;
        } else if c == '}' {
            d -= 1;
        }
        if c == ',' && d == 0 {
            options.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(c);
    }
    options.push(cur);
    if options.len() == 1 {
        return vec![pattern.to_string()];
    }
    let mut out: Vec<String> = vec![];
    for o in options {
        for e in expand_braces(&format!("{prefix}{o}{suffix}")) {
            if !out.contains(&e) {
                out.push(e);
            }
            if out.len() >= MAX_BRACE_VARIANTS {
                return out;
            }
        }
    }
    out
}

fn matcher(p: &str) -> Option<GlobMatcher> {
    GlobBuilder::new(p).literal_separator(true).backslash_escape(true).build().ok().map(|g| g.compile_matcher())
}

fn rank(rel: &str) -> (u8, String) {
    (if rel.split('/').any(|p| p.starts_with('.')) { 1 } else { 0 }, rel.to_string())
}

fn has_wildcard(s: &str) -> bool {
    s.contains(['*', '?', '[', ']'])
}

fn literal_prefix(p: &str) -> Vec<String> {
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let mut out = vec![];
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 || has_wildcard(part) {
            break;
        }
        out.push(part.to_string());
    }
    out
}

fn shared_prefix(prefixes: &[Vec<String>]) -> Vec<String> {
    let Some(first) = prefixes.first() else {
        return vec![];
    };
    let mut out = vec![];
    for (i, seg) in first.iter().enumerate() {
        if prefixes.iter().all(|p| p.get(i) == Some(seg)) {
            out.push(seg.clone());
        } else {
            break;
        }
    }
    out
}

/// Every visible file under `root` as `(path relative to base, path)`, in
/// depth-first order. Directories are read in parallel: the walk is mostly
/// `readdir` syscalls, and results are ranked and sorted afterwards anyway.
/// Stops descending once `expired`; returns `(files, timed_out)`.
fn visible_files(
    root: &Path,
    base: &Path,
    gi: &ignore::gitignore::Gitignore,
    allowed_noise: &BTreeSet<String>,
    expired: &(dyn Fn() -> bool + Sync),
) -> (Vec<(String, PathBuf)>, bool) {
    let timed_out = std::sync::atomic::AtomicBool::new(false);
    let found = walk_dir(root, base, gi, allowed_noise, expired, &timed_out);
    (found, timed_out.into_inner())
}

fn walk_dir(
    dir: &Path,
    base: &Path,
    gi: &ignore::gitignore::Gitignore,
    allowed_noise: &BTreeSet<String>,
    expired: &(dyn Fn() -> bool + Sync),
    timed_out: &std::sync::atomic::AtomicBool,
) -> Vec<(String, PathBuf)> {
    use rayon::prelude::*;
    if expired() {
        timed_out.store(true, std::sync::atomic::Ordering::Relaxed);
        return vec![];
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let rel_dir = dir.strip_prefix(base).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
    let join = |n: &str| {
        if rel_dir.is_empty() {
            n.to_string()
        } else {
            format!("{rel_dir}/{n}")
        }
    };
    let mut dirs = vec![];
    let mut files = vec![];
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        match e.file_type() {
            Ok(t) if t.is_dir() => dirs.push(name),
            Ok(_) => files.push(name),
            Err(_) => {}
        }
    }
    files.sort();
    let mut found: Vec<(String, PathBuf)> = files
        .into_iter()
        .filter_map(|f| {
            let rel = join(&f);
            (!is_gitignored(gi, &rel, false)).then(|| (rel, dir.join(&f)))
        })
        .collect();
    let mut kept: Vec<String> = dirs.into_iter().filter(|d| (!NOISE_DIR_NAMES.contains(&d.as_str()) || allowed_noise.contains(d)) && !is_gitignored(gi, &join(d), true)).collect();
    kept.sort_by(|a, b| (a.starts_with('.'), a).cmp(&(b.starts_with('.'), b)));
    let subs: Vec<Vec<(String, PathBuf)>> = kept.par_iter().map(|d| walk_dir(&dir.join(d), base, gi, allowed_noise, expired, timed_out)).collect();
    found.extend(subs.into_iter().flatten());
    found
}

/// The tool's text, noting when the walk stopped early.
fn partial_note(mut text: String, timed_out: bool) -> String {
    if timed_out {
        text.push_str(&format!("\n\n[glob stopped after {GLOB_TIMEOUT_S}s: these results are partial. Narrow the directory or pattern for a complete list.]"));
    }
    text
}

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }
    async fn run(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        let mut a = Args::new("glob", &args);
        let pattern = a.req_str(&["pattern", "glob"]);
        let directory = a.str_or(&["directory", "dir", "path"], ".");
        let mode = a.literal(&["match"], &["path", "name"], "path");
        let max_results = a.opt_int(&["max_results"], Some(1), None).unwrap_or(200) as usize;
        a.finish()?;
        let denied = ctx.denied.clone();
        let resolved = denied.validate_read_path(&directory)?;
        if !resolved.is_dir() {
            return Err(ToolError::Execution(format!("Not a directory: {}", denied.display_path(&resolved))));
        }
        if mode == "path" {
            if Path::new(&pattern).is_absolute() || pattern.starts_with('/') {
                return Err(ToolError::Execution(format!("Pattern must be relative to the search directory: {}", crate::py_repr_str(&pattern))));
            }
            if pattern.split('/').any(|p| p == "..") {
                return Err(ToolError::Execution(format!(
                    "'..' is not allowed in a pattern: {} — pass the 'directory' argument to search somewhere else.",
                    crate::py_repr_str(&pattern)
                )));
            }
        }
        let pat = pattern.clone();
        let (hits, dir_hints, timed_out) = tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(GLOB_TIMEOUT_S);
            let gi = load_gitignore(&resolved);
            let variants = expand_braces(&pat);
            let allowed_noise: BTreeSet<String> =
                variants.iter().flat_map(|v| v.split('/').map(String::from).collect::<Vec<_>>()).filter(|s| NOISE_DIR_NAMES.contains(&s.as_str()) && s != ".git").collect();
            let prefix = if mode == "path" { shared_prefix(&variants.iter().map(|v| literal_prefix(v)).collect::<Vec<_>>()) } else { vec![] };
            let walk_root = prefix.iter().fold(resolved.clone(), |p, s| p.join(s));
            if !prefix.is_empty() {
                let mut rel = String::new();
                for seg in &prefix {
                    rel = if rel.is_empty() { seg.clone() } else { format!("{rel}/{seg}") };
                    if (NOISE_DIR_NAMES.contains(&seg.as_str()) && !allowed_noise.contains(seg)) || is_gitignored(&gi, &rel, true) {
                        return (vec![], vec![], false);
                    }
                }
            }
            if !walk_root.is_dir() {
                return (vec![], vec![], false);
            }
            let (cands, timed_out) = visible_files(&walk_root, &resolved, &gi, &allowed_noise, &|| std::time::Instant::now() > deadline);
            let select = |pats: &[String]| -> Vec<(String, PathBuf)> {
                let mut hit: Vec<(String, PathBuf)> = if mode == "name" {
                    let rxs: Vec<regex::Regex> = pats.iter().filter_map(|p| regex::Regex::new(&crate::denied::fnmatch_translate(p)).ok()).collect();
                    cands.iter().filter(|(_, p)| rxs.iter().any(|r| r.is_match(&p.file_name().unwrap_or_default().to_string_lossy()))).cloned().collect()
                } else if pats.iter().any(|p| p.ends_with('/')) {
                    vec![]
                } else {
                    let ms: Vec<GlobMatcher> = pats.iter().filter_map(|p| matcher(p)).collect();
                    cands.iter().filter(|(r, _)| ms.iter().any(|m| m.is_match(r))).cloned().collect()
                };
                hit.sort_by_cached_key(|(r, _)| rank(r));
                hit
            };
            let mut matched = select(&variants);
            if matched.is_empty() {
                let widened: Vec<String> = variants.iter().filter(|p| !p.contains('/') && !p.starts_with("**")).map(|p| format!("**/{p}")).collect();
                if !widened.is_empty() {
                    matched = select(&widened);
                }
            }
            let mut hits = vec![];
            for (_, p) in matched {
                if !p.is_file() || denied.is_denied_read_path(&p) {
                    continue;
                }
                hits.push(denied.display_path(&p));
                if hits.len() >= max_results {
                    break;
                }
            }
            if !hits.is_empty() || mode == "name" {
                return (hits, vec![], timed_out);
            }
            let mut dirs: BTreeSet<String> = BTreeSet::new();
            for (rel, _) in &cands {
                let parts: Vec<&str> = rel.split('/').collect();
                for i in 1..parts.len() {
                    dirs.insert(parts[..i].join("/"));
                }
            }
            let ms: Vec<GlobMatcher> = variants.iter().filter_map(|p| matcher(p)).collect();
            (hits, dirs.into_iter().filter(|d| ms.iter().any(|m| m.is_match(d))).collect(), timed_out)
        })
        .await
        .map_err(ToolError::exec)?;
        if hits.is_empty() {
            let miss = format!("No files matching '{pattern}' in {}", ctx.denied.display_path(&ctx.denied.validate_read_path(&directory)?));
            if !dir_hints.is_empty() {
                let named = dir_hints.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
                return Ok(ToolOutput::Text(partial_note(format!("{miss}; it matches directories ({named}) — use '{}/**' to list files inside", dir_hints[0]), timed_out)));
            }
            return Ok(ToolOutput::Text(partial_note(miss, timed_out)));
        }
        Ok(ToolOutput::Text(partial_note(hits.join("\n"), timed_out)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braces_expand() {
        assert_eq!(expand_braces("src/**/*.{ts,tsx}"), vec!["src/**/*.ts", "src/**/*.tsx"]);
        assert_eq!(expand_braces("b{.py,}"), vec!["b.py", "b"]);
        assert_eq!(expand_braces("x{"), vec!["x{"]);
    }

    #[test]
    fn globs_like_pathlib() {
        let m = matcher("**/*.py").unwrap();
        assert!(m.is_match("a.py"));
        assert!(m.is_match("a/b/c.py"));
        let m = matcher("src/*.py").unwrap();
        assert!(!m.is_match("src/a/b.py"));
    }

    fn glob_tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for f in ["b.txt", "a.txt", ".hidden/c.txt", "sub/d.txt", "sub/deep/e.txt", "node_modules/x.txt", "ignored.txt", "src/m.rs", "src/n.rs"] {
            let p = d.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        }
        std::fs::write(d.path().join(".gitignore"), "ignored.txt\n").unwrap();
        d
    }

    async fn glob(ws: &Path, args: Value) -> Vec<String> {
        let ctx = crate::ToolContext {
            session_id: None,
            agent_name: "t".into(),
            tool_call_id: "c".into(),
            denied: std::sync::Arc::new(crate::denied::DeniedPaths::with(ws, None, Some(vec![]), Some(vec![]))),
            workspace: None,
            output: None,
            metadata: Default::default(),
            messages: None,
        };
        match GlobTool.run(&ctx, args).await.unwrap() {
            ToolOutput::Text(t) => t.lines().map(|l| l.replace('\\', "/")).collect(),
            o => panic!("{o:?}"),
        }
    }

    #[tokio::test]
    async fn ranks_visible_paths_first_and_honours_max() {
        let d = glob_tree();
        let all = glob(d.path(), serde_json::json!({"pattern": "**/*.txt"})).await;
        assert_eq!(all, ["a.txt", "b.txt", "sub/d.txt", "sub/deep/e.txt", ".hidden/c.txt"]);
        assert_eq!(glob(d.path(), serde_json::json!({"pattern": "**/*.txt", "max_results": 2})).await, ["a.txt", "b.txt"]);
        assert_eq!(glob(d.path(), serde_json::json!({"pattern": "src/*.{rs,txt}"})).await, ["src/m.rs", "src/n.rs"]);
    }

    #[tokio::test]
    async fn widens_bare_names_and_opens_named_noise_dirs() {
        let d = glob_tree();
        assert_eq!(glob(d.path(), serde_json::json!({"pattern": "e.txt"})).await, ["sub/deep/e.txt"]);
        assert_eq!(glob(d.path(), serde_json::json!({"pattern": "node_modules/*.txt"})).await, ["node_modules/x.txt"]);
        assert_eq!(glob(d.path(), serde_json::json!({"pattern": "c.txt", "match": "name"})).await, [".hidden/c.txt"]);
    }

    #[tokio::test]
    async fn misses_name_matching_directories() {
        let d = glob_tree();
        let miss = glob(d.path(), serde_json::json!({"pattern": "ignored.txt"})).await;
        assert!(miss[0].starts_with("No files matching 'ignored.txt'"), "{miss:?}");
        let dirs = glob(d.path(), serde_json::json!({"pattern": "s*"})).await;
        assert!(dirs[0].contains("it matches directories (src, sub)") && dirs[0].contains("use 'src/**'"), "{dirs:?}");
    }

    #[test]
    fn a_timed_out_walk_keeps_what_it_found() {
        let d = glob_tree();
        let gi = load_gitignore(d.path());
        let (all, timed_out) = visible_files(d.path(), d.path(), &gi, &BTreeSet::new(), &|| false);
        assert!(!timed_out);
        assert_eq!(all.len(), 8, "{all:?}");
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let (partial, timed_out) = visible_files(d.path(), d.path(), &gi, &BTreeSet::new(), &|| calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 2);
        assert!(timed_out);
        assert!(!partial.is_empty() && partial.len() < all.len(), "{partial:?}");
        assert!(partial.iter().all(|p| all.contains(p)));
        assert_eq!(
            partial_note("a.txt".into(), true),
            format!("a.txt\n\n[glob stopped after {GLOB_TIMEOUT_S}s: these results are partial. Narrow the directory or pattern for a complete list.]")
        );
        assert_eq!(partial_note("a.txt".into(), false), "a.txt");
    }
}
