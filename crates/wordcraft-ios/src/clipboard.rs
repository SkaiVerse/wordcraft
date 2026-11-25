//! iPadOS `UIPasteboard` + rich-flavor memory clipboard backend for WordCraft.

use craft::platform::clipboard::{ClipboardBackend, ClipboardError, ClipboardImage, Flavor, TEXT};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct ClipboardState {
    flavors: Vec<Flavor>,
    system_text: String,
    outgoing: VecDeque<Value>,
}

/// Clipboard handle bridging WordCraft rich-text flavors with iPadOS `UIPasteboard`.
#[derive(Clone, Default)]
pub struct IosClipboard {
    state: Arc<Mutex<ClipboardState>>,
}

impl IosClipboard {
    /// Creates a new shared iOS clipboard bridge.
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates plain text synced from `UIPasteboard.general.string`.
    /// When the OS pasteboard holds newer text from another app than what WordCraft last wrote,
    /// clears stale internal rich flavors so Paste uses the external text instead.
    pub fn sync_from_system(&self, text: &str) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.system_text != text {
            st.system_text = text.to_string();
            let matches_ours = st.flavors.iter().find(|f| f.mime == TEXT).is_some_and(|f| std::str::from_utf8(&f.data).ok() == Some(text));
            if !matches_ours {
                st.flavors.clear();
                if !text.is_empty() {
                    st.flavors.push(Flavor::text(text));
                }
            }
        }
    }

    /// Pops the next queued pasteboard write action for Swift.
    pub fn pop_action(&self) -> Option<Value> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).outgoing.pop_front()
    }
}

impl ClipboardBackend for IosClipboard {
    fn write(&mut self, flavors: &[Flavor]) -> Result<usize, ClipboardError> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.flavors = flavors.to_vec();
        if let Some(text_flavor) = flavors.iter().find(|f| f.mime == TEXT) {
            if let Ok(text) = std::str::from_utf8(&text_flavor.data) {
                st.system_text = text.to_string();
                st.outgoing.push_back(json!({
                    "action": "clipboard.set",
                    "text": text,
                }));
            }
        } else if flavors.is_empty() {
            st.system_text.clear();
            st.outgoing.push_back(json!({
                "action": "clipboard.set",
                "text": "",
            }));
        }
        Ok(0)
    }

    fn read(&mut self, mimes: &[&str], limit: u64) -> Result<Option<Flavor>, ClipboardError> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for mime in mimes {
            if let Some(flavor) = st.flavors.iter().find(|f| f.mime == *mime) {
                if flavor.data.len() as u64 > limit {
                    return Err(ClipboardError::TooLarge { limit });
                }
                return Ok(Some(flavor.clone()));
            }
        }
        Ok(None)
    }

    fn read_image(&mut self, limit: u64) -> Result<Option<ClipboardImage>, ClipboardError> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = st.flavors.iter().find(|f| f.mime == craft::platform::clipboard::RGBA8) {
            if f.data.len() as u64 > limit {
                return Err(ClipboardError::TooLarge { limit });
            }
            return Ok(f.rgba8_image());
        }
        Ok(None)
    }

    fn read_files(&mut self, _limit: usize) -> Result<Option<Vec<PathBuf>>, ClipboardError> {
        Ok(None)
    }

    fn has(&mut self, mimes: &[&str]) -> bool {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        mimes.iter().any(|m| st.flavors.iter().any(|f| f.mime == *m))
    }

    fn formats(&mut self) -> Vec<String> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.flavors.iter().map(|f| f.mime.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_queues_pasteboard_and_external_sync_replaces_stale_flavors() {
        let mut cb = IosClipboard::new();
        cb.write(&[Flavor::text("internal copy")]).expect("write");
        let act = cb.pop_action().expect("queued set");
        assert_eq!(act["action"], "clipboard.set");
        assert_eq!(act["text"], "internal copy");

        cb.sync_from_system("from safari");
        let read = cb.read(&[TEXT], 1024).expect("read").expect("text flavor");
        assert_eq!(std::str::from_utf8(&read.data).unwrap(), "from safari");
    }
}
