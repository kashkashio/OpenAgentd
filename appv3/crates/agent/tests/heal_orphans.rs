//! `heal_orphaned_tool_calls`: every tool call in the LLM window gets a
//! result row before the next turn, so providers never see a dangling call.

use appv3_agent::history::{heal_orphaned_tool_calls, INTERRUPTED_TOOL_RESULT};
use appv3_db::queries::*;
use serde_json::json;

async fn fresh() -> (tempfile::TempDir, appv3_db::DbPool, String) {
    let dir = tempfile::tempdir().unwrap();
    let pool = appv3_db::create_pool(dir.path().join("oad.db")).await.unwrap();
    let s = create_session(&pool, NewSession { workspace: "/tmp/ws".into(), ..Default::default() }).await.unwrap();
    (dir, pool, s.id)
}

fn call(id: &str, name: &str) -> serde_json::Value {
    json!({"id": id, "type": "function", "function": {"name": name, "arguments": "{}"}})
}

async fn assistant_calling(pool: &appv3_db::DbPool, sid: &str, calls: &[(&str, &str)]) {
    let tool_calls = json!(calls.iter().map(|(id, name)| call(id, name)).collect::<Vec<_>>());
    save_message(pool, sid, NewMessage { tool_calls: Some(tool_calls), ..NewMessage::assistant(Some("calling".into())) }).await.unwrap();
}

/// `(role, tool_call_id, content)` in transcript order.
async fn transcript(pool: &appv3_db::DbPool, sid: &str) -> Vec<(String, Option<String>, Option<String>)> {
    let rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as("SELECT role, tool_call_id, content FROM session_messages WHERE session_id = ? ORDER BY seq, id")
        .bind(appv3_db::codec::db_id(sid))
        .fetch_all(pool)
        .await
        .unwrap();
    rows
}

#[tokio::test]
async fn orphaned_calls_get_an_interrupted_result_right_after_their_assistant() {
    let (_d, pool, sid) = fresh().await;
    save_message(&pool, &sid, NewMessage::user("go")).await.unwrap();
    assistant_calling(&pool, &sid, &[("c1", "read"), ("c2", "grep")]).await;
    save_message(&pool, &sid, NewMessage::tool("c1", "read", "file body")).await.unwrap();
    save_message(&pool, &sid, NewMessage::user("next")).await.unwrap();

    assert_eq!(heal_orphaned_tool_calls(&pool, &sid).await.unwrap(), 1);
    let t = transcript(&pool, &sid).await;
    let tail: Vec<_> = t.iter().map(|(role, id, _)| (role.as_str(), id.as_deref())).collect();
    assert_eq!(tail, vec![("user", None), ("assistant", None), ("tool", Some("c2")), ("tool", Some("c1")), ("user", None)]);
    assert_eq!(t[2].2.as_deref(), Some(INTERRUPTED_TOOL_RESULT));
    assert_eq!(heal_orphaned_tool_calls(&pool, &sid).await.unwrap(), 0, "a second pass finds nothing to heal");
}

#[tokio::test]
async fn fully_answered_calls_are_left_alone() {
    let (_d, pool, sid) = fresh().await;
    assistant_calling(&pool, &sid, &[("c1", "read")]).await;
    save_message(&pool, &sid, NewMessage::tool("c1", "read", "ok")).await.unwrap();
    assert_eq!(heal_orphaned_tool_calls(&pool, &sid).await.unwrap(), 0);
    assert_eq!(transcript(&pool, &sid).await.len(), 2);
}

#[tokio::test]
async fn orphans_hidden_behind_a_summary_are_not_healed() {
    let (_d, pool, sid) = fresh().await;
    assistant_calling(&pool, &sid, &[("old", "read")]).await;
    save_message(&pool, &sid, NewMessage { is_summary: true, ..NewMessage::user("summary of earlier work") }).await.unwrap();
    save_message(&pool, &sid, NewMessage::user("after the summary")).await.unwrap();
    assert_eq!(heal_orphaned_tool_calls(&pool, &sid).await.unwrap(), 0);
}
