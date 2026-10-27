//! `notifications/progress` for a tool call that asked for it.
//!
//! A client that passes `_meta.progressToken` on `tools/call` is told how
//! far a long call has come, on the same pipe as the answers (the stdio
//! writer serialises both). MCP lets a client reset its own per-call
//! timeout on each notification, which is what keeps a large upload from
//! reading as a dead server to a client with a budget per call. A call
//! without a token, or a server driven without a writer (the tests'
//! direct `handle_request`), reports nothing.

use serde_json::{json, Value};
use tokio::sync::mpsc;

/// Where a call's progress goes, if anywhere.
#[derive(Clone, Default)]
pub struct Progress {
    token: Option<Value>,
    out: Option<mpsc::UnboundedSender<String>>,
}

impl Progress {
    /// The progress of a `tools/call` whose params are `params`, written
    /// to `out`: silent unless the call carries `_meta.progressToken`.
    pub fn for_call(params: Option<&Value>, out: Option<mpsc::UnboundedSender<String>>) -> Self {
        let token = params
            .and_then(|p| p.get("_meta"))
            .and_then(|m| m.get("progressToken"))
            .filter(|t| t.is_string() || t.is_number())
            .cloned();
        Self { token, out }
    }

    /// Report `done` of `total` with a short `message`. A closed pipe is
    /// not an error here: the answer that follows meets it anyway.
    pub fn report(&self, done: u64, total: u64, message: &str) {
        let (Some(token), Some(out)) = (&self.token, &self.out) else {
            return;
        };
        let line = json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {
                "progressToken": token,
                "progress": done,
                "total": total,
                "message": message,
            }
        });
        let _ = out.send(line.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_with_a_token_reports_on_the_pipe() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let params = json!({ "name": "sftp_upload", "_meta": { "progressToken": 7 } });
        Progress::for_call(Some(&params), Some(tx)).report(10, 40, "uploading");
        let sent: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(sent["method"], "notifications/progress");
        assert_eq!(sent["params"]["progressToken"], 7);
        assert_eq!(sent["params"]["progress"], 10);
        assert_eq!(sent["params"]["total"], 40);
        assert!(sent.get("id").is_none(), "a notification carries no id");
    }

    #[test]
    fn a_call_without_a_token_reports_nothing() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        Progress::for_call(Some(&json!({ "name": "sftp_upload" })), Some(tx)).report(1, 2, "x");
        assert!(rx.try_recv().is_err());
    }
}
