//! `openagentd import claude-code`: bring Claude Code sessions into
//! OpenAgentd (see `appv3_agent::claude_code_import`).

use crate::cli::ClaudeCodeImportArgs;
use crate::ui::{bold, dim, green, red, yellow};
use anyhow::{bail, Context, Result};
use appv3_agent::claude_code_import::{default_root, import, ImportOptions};

pub fn claude_code(a: &ClaudeCodeImportArgs) -> Result<()> {
    let root = match a.from.clone().or_else(default_root) {
        Some(r) => r,
        None => bail!("No home directory; pass --from <dir>."),
    };
    if !root.is_dir() {
        bail!("No Claude Code transcripts at {} (pass --from <dir>).", root.display());
    }
    let db_path = appv3_core::settings().database_path.clone();
    let opts = ImportOptions { root: root.clone(), project: a.project.clone(), subagents: !a.no_subagents, workflows: a.workflows && !a.no_subagents, dry_run: a.dry_run };
    println!();
    println!("  {}  {} Claude Code sessions from {}", dim("…"), if a.dry_run { "Checking" } else { "Importing" }, root.display());
    println!("  {}  Database: {}", dim("→"), db_path.display());
    println!();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let report = rt
        .block_on(async {
            let pool = appv3_db::create_pool(&db_path).await?;
            let r = import(&pool, &opts).await;
            pool.close().await;
            r
        })
        .with_context(|| format!("import into {}", db_path.display()))?;

    for s in &report.sessions {
        let mark = match s.status.as_str() {
            "new" => green("  +"),
            "updated" => green("  ↑"),
            "skipped" => yellow("  –"),
            "error" => red("  ✗"),
            _ => dim("  ="),
        };
        let counts = match s.status.as_str() {
            "new" | "updated" => {
                let sub = if s.subagents > 0 { format!(", {} sub-agents", s.subagents) } else { String::new() };
                dim(&format!("{} messages{sub}", s.messages))
            }
            "skipped" => dim("OpenAgentd's own session"),
            "error" => red(s.detail.as_deref().unwrap_or("failed")),
            _ => dim("up to date"),
        };
        let title = if s.title.is_empty() { s.id.as_str() } else { s.title.as_str() };
        println!("  {mark} {}  {counts}", bold(title));
        println!("      {}", dim(&s.workspace));
    }
    println!();
    let verb = if report.dry_run { "Would import" } else { "Imported" };
    println!("  {}  {verb} {} messages and {} sub-agent sessions across {} workspaces.", green("✓"), report.messages, report.subagents, report.workspaces);
    if report.dry_run {
        println!("     {}", dim("Run again without --dry-run to write them."));
    } else {
        println!("     {}", dim("Refresh OpenAgentd to see them in the sidebar."));
    }
    println!();
    Ok(())
}
