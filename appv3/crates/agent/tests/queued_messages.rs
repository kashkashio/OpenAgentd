//! Queued user messages ("steers") keep the order they were sent in.

use appv3_agent::loader::ProviderFactory;
use appv3_agent::session::{AgentSession, UserMessage};
use appv3_agent::{store, Agent};
use appv3_providers::mock::MockProvider;
use appv3_providers::{ChatMessage, LlmProvider};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Settings are process-wide, so every test shares one set of roots.
fn root() -> &'static Path {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().unwrap();
        for (k, d) in [
            ("OPENAGENTD_DATA_DIR", "data"),
            ("OPENAGENTD_CONFIG_DIR", "config"),
            ("OPENAGENTD_STATE_DIR", "state"),
            ("OPENAGENTD_CACHE_DIR", "cache"),
            ("OPENAGENTD_WORKSPACE_DIR", "ws"),
        ] {
            std::env::set_var(k, root.path().join(d));
        }
        std::env::set_var("HOME", root.path().join("home"));
        appv3_core::settings::install(appv3_core::settings::Settings::from_env());
        root
    })
    .path()
}

struct Harness {
    pool: appv3_db::DbPool,
    session: Arc<AgentSession>,
    mock: Arc<MockProvider>,
    sid: String,
}

async fn harness(name: &str) -> Harness {
    let root = root();
    let pool = appv3_db::create_pool(root.join(format!("{name}.db"))).await.unwrap();
    let ws = root.join(name);
    std::fs::create_dir_all(&ws).unwrap();
    let mock = Arc::new(MockProvider::new(vec![MockProvider::text("First."), MockProvider::text("Second.")]));
    let provider: Arc<dyn LlmProvider> = mock.clone();
    let p2 = provider.clone();
    let factory: ProviderFactory = Arc::new(move |_, _| Ok(p2.clone()));
    let agent = Agent::new(provider, name, "You are a test agent.", vec![], Some("mock:mock".into()));
    let session = AgentSession::new(agent, None, Some(ws.display().to_string()), pool.clone(), factory, None);
    Harness { pool, session, mock, sid: uuid::Uuid::now_v7().to_string() }
}

impl Harness {
    async fn send(&self, text: &str) {
        self.session.handle_user_message(UserMessage { content: text.into(), session_id: self.sid.clone(), origin: "user".into(), ..Default::default() }).await.unwrap();
        if let Some(mut sub) = store().attach(&self.sid) {
            let _ = tokio::time::timeout(Duration::from_secs(10), async { while sub.next().await.is_some() {} }).await;
        }
        tokio::time::timeout(Duration::from_secs(10), self.session.wait_turn_finished()).await.expect("turn finishes");
    }

    /// User texts of the model call at `index`, in the order the model read them.
    fn user_texts(&self, index: usize) -> Vec<String> {
        let calls = self.mock.calls.lock().unwrap();
        let (messages, _, _) = calls.get(index).expect("the model was called");
        messages
            .iter()
            .filter_map(|m| match m {
                ChatMessage::User { content, .. } => content.clone(),
                _ => None,
            })
            .collect()
    }

    /// Visible user rows in transcript order.
    async fn stored_user_texts(&self) -> Vec<String> {
        let rows = appv3_db::llm_window_rows(&self.pool, &self.sid, false).await.unwrap();
        rows.into_iter().filter(|r| r.role == "user" && r.kind != "note").filter_map(|r| r.content).collect()
    }
}

/// A turn that fails leaves its unread steers queued. The next message must
/// not overtake them: the model reads them, and the transcript stores them,
/// in send order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_message_does_not_overtake_steers_left_queued() {
    let h = harness("overtake").await;
    h.send("start").await;
    // As after a failed turn: queued, with no turn left to read it.
    appv3_db::save_queued_user_message(&h.pool, &h.sid, "older steer", None).await.unwrap();

    h.send("newer message").await;

    // Consecutive user turns may be merged into one message; order is what counts.
    let read = h.user_texts(1).join("\n\n");
    let (older, newer) = (read.find("older steer").expect("steer read"), read.find("newer message").expect("message read"));
    assert!(older < newer, "the model read the newer message first: {read:?}");
    assert_eq!(h.stored_user_texts().await, ["start", "older steer", "newer message"]);
}
