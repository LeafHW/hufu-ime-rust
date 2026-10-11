use hufu_types::CandidateMenu;
use serde_json::{json, Value};

pub struct Export {
    pub owner: String,
    instance: String,
    version: u64,
    menu: Option<CandidateMenu>,
    last_owner: String,
}
impl Default for Export {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            owner: String::new(),
            instance: format!(
                "{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ),
            version: 0,
            menu: None,
            last_owner: String::new(),
        }
    }
}
impl Export {
    pub fn response(&mut self, req: &Value, menu: Option<CandidateMenu>) -> Value {
        let context = req
            .pointer("/candidate_ui/context")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 128)
            .unwrap_or("");
        // A foreign query cannot claim or invalidate the owner's cached menu.
        if context.is_empty() || context != self.owner {
            return json!({"protocol":1,"instance":self.instance,"context":context,"version":self.version,"menu":null});
        }
        if self.menu != menu || self.last_owner != self.owner || self.version == 0 {
            self.version += 1;
            self.menu = menu;
            self.last_owner = self.owner.clone();
        }
        let mut out =
            json!({"protocol":1,"instance":self.instance,"context":context,"version":self.version});
        if req
            .pointer("/candidate_ui/instance")
            .and_then(Value::as_str)
            != Some(self.instance.as_str())
            || req.pointer("/candidate_ui/version").and_then(Value::as_u64) != Some(self.version)
        {
            out["menu"] = json!(self.menu);
        }
        if serde_json::to_vec(&out).map_or(true, |v| v.len() > 1 << 20) {
            out["menu"] = Value::Null;
        }
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_versions_and_reconnect() {
        let mut e = Export::default();
        e.owner = "a".into();
        let mut req = json!({"candidate_ui":{"context":"a"}});
        let menu = Some(CandidateMenu {
            texts: vec!["𠀀é".into()],
            pages: vec![0],
            selected: 0,
            page: 0,
        });
        let a = e.response(&req, menu.clone());
        assert!(a["menu"].is_object());
        req["candidate_ui"]["instance"] = a["instance"].clone();
        req["candidate_ui"]["version"] = a["version"].clone();
        assert!(e.response(&req, menu.clone()).get("menu").is_none());
        assert!(e.response(&json!({"candidate_ui":{"context":"b"}}), None)["menu"].is_null());
        assert!(e.response(&req, menu).get("menu").is_none());
        assert!(e.response(&req, None)["menu"].is_null());
        assert_ne!(Export::default().instance, e.instance);
    }
}
