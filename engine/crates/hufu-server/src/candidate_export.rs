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
        let mut menu_included = false;
        if req
            .pointer("/candidate_ui/instance")
            .and_then(Value::as_str)
            != Some(self.instance.as_str())
            || req.pointer("/candidate_ui/version").and_then(Value::as_u64) != Some(self.version)
        {
            out["menu"] = json!(self.menu);
            menu_included = true;
        }
        // 【T3 2026-11-07】原实现整帧 to_vec 量尺寸、发送侧再序列化一遍
        // （MB 级菜单=双重序列化）；只对已附带的 menu 单独量尺寸，无
        // menu 的帧本就只有几百字节，不需要量。
        if menu_included
            && self
                .menu
                .as_ref()
                .and_then(|m| serde_json::to_vec(m).ok())
                .map_or(true, |v| v.len() > 1 << 20)
        {
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
