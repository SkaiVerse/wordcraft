//! Native iPadOS `Files` service bridging WordCraft's non-blocking file picker requests
//! to UIKit `UIDocumentPickerViewController` and handling security-scoped file reads/writes.

use craft::host::{FileError, Files, PickId, PickKind, PickRequest, Picked, WriteIntent};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Pending platform action queued from Rust for Swift UIKit execution.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingFileAction {
    pub pick_id: u64,
    pub request: PickRequest,
}

#[derive(Default)]
struct FilesState {
    next_id: u64,
    pending_picks: Vec<PickId>,
    pick_purposes: HashMap<PickId, String>,
    answered: HashMap<PickId, Picked>,
    pending_saves: HashMap<PickId, Vec<u8>>,
    last_folders: HashMap<String, PathBuf>,
    outgoing_actions: VecDeque<Value>,
}

/// Shared `craft::host::Files` implementation for iPadOS.
#[derive(Clone, Default)]
pub struct IosFiles {
    state: Arc<Mutex<FilesState>>,
}

impl IosFiles {
    /// Creates a new iOS file service.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pops the next queued UIKit file-picker request as JSON, if any.
    pub fn pop_action(&self) -> Option<Value> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).outgoing_actions.pop_front()
    }

    /// Completes an active file picker ticket with the picked file paths (empty = cancelled).
    pub fn complete_pick(&self, pick_id: u64, paths: Vec<PathBuf>) {
        let id = PickId(pick_id);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending_picks.retain(|&p| p != id);
        let purpose = st.pick_purposes.remove(&id).unwrap_or_default();
        let pending_save = st.pending_saves.remove(&id);

        let picked = if paths.is_empty() {
            Picked::Cancelled
        } else {
            if let Some(bytes) = pending_save
                && let Some(dest) = paths.first()
                && let Err(err) = write_security_scoped(dest, &bytes)
            {
                st.answered.insert(id, Picked::Failed(err.to_string()));
                return;
            }
            if let Some(parent) = paths.first().and_then(|p| p.parent())
                && !purpose.is_empty()
            {
                st.last_folders.insert(purpose, parent.to_path_buf());
            }
            Picked::Paths(paths)
        };

        st.answered.insert(id, picked);
    }

    /// Completes an active file picker ticket with an error message.
    pub fn fail_pick(&self, pick_id: u64, reason: String) {
        let id = PickId(pick_id);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending_picks.retain(|&p| p != id);
        st.pick_purposes.remove(&id);
        st.pending_saves.remove(&id);
        st.answered.insert(id, Picked::Failed(reason));
    }

    fn enqueue_pick(&self, request: PickRequest, save_bytes: Option<Vec<u8>>) -> PickId {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.next_id += 1;
        let id = PickId(st.next_id);
        st.pending_picks.push(id);
        st.pick_purposes.insert(id, request.purpose.clone());
        if let Some(bytes) = save_bytes {
            st.pending_saves.insert(id, bytes);
        }

        let kind_str = match request.kind {
            PickKind::Open => "open",
            PickKind::OpenMany => "open_many",
            PickKind::Folder => "folder",
            PickKind::Save => "save",
        };
        let extensions: Vec<String> = request.filters.iter().flat_map(|f| f.extensions.iter().cloned()).collect();
        let default_name = request
            .file_name
            .clone()
            .unwrap_or_else(|| if let Some(ext) = &request.suggested_extension { format!("Document.{ext}") } else { "Document.docx".to_string() });

        st.outgoing_actions.push_back(json!({
            "action": "file.pick",
            "pick_id": id.0,
            "kind": kind_str,
            "purpose": request.purpose,
            "title": request.title,
            "file_name": default_name,
            "suggested_extension": request.suggested_extension,
            "extensions": extensions,
        }));
        id
    }
}

fn map_io_error(err: std::io::Error, path: &Path) -> FileError {
    match err.kind() {
        std::io::ErrorKind::NotFound => FileError::NotFound(path.to_path_buf()),
        std::io::ErrorKind::PermissionDenied => FileError::PermissionDenied(path.to_path_buf()),
        _ => FileError::Other(err.to_string()),
    }
}

/// Writes `bytes` directly without sibling `.tmp` creation if sandbox denies directory writes,
/// because iPadOS security-scoped file URLs from `UIDocumentPickerViewController` grant write
/// permission to the chosen file URL itself rather than creating sibling files in its parent.
fn write_security_scoped(path: &Path, bytes: &[u8]) -> Result<(), FileError> {
    std::fs::write(path, bytes).map_err(|e| map_io_error(e, path))
}

impl Files for IosFiles {
    fn pick(&self, request: PickRequest) -> PickId {
        self.enqueue_pick(request, None)
    }

    fn take_answer(&self, id: PickId) -> Option<Picked> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let picked = st.answered.remove(&id)?;
        if let Picked::Paths(ref paths) = picked
            && let Some(parent) = paths.first().and_then(|p| p.parent())
        {
            st.last_folders.entry("document".to_string()).or_insert_with(|| parent.to_path_buf());
        }
        Some(picked)
    }

    fn forget(&self, id: PickId) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending_picks.retain(|&p| p != id);
        st.pick_purposes.remove(&id);
        st.answered.remove(&id);
        st.pending_saves.remove(&id);
    }

    fn pick_pending(&self) -> bool {
        !self.state.lock().unwrap_or_else(|e| e.into_inner()).pending_picks.is_empty()
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>, FileError> {
        std::fs::read(path).map_err(|e| map_io_error(e, path))
    }

    fn write(&self, path: &Path, bytes: &[u8], _intent: WriteIntent) -> Result<bool, FileError> {
        write_security_scoped(path, bytes)?;
        Ok(false)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn last_folder(&self, purpose: &str) -> Option<PathBuf> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).last_folders.get(purpose).cloned()
    }

    fn save(&self, request: PickRequest, bytes: Vec<u8>) -> PickId {
        self.enqueue_pick(request, Some(bytes))
    }

    fn can_pick(&self) -> bool {
        true
    }

    fn browser_saves(&self) -> bool {
        false
    }

    fn answer_next(&self, picked: Picked) -> Result<(), FileError> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let id = st.pending_picks.first().copied().ok_or(FileError::Other("no pending pick".into()))?;
        st.pending_picks.retain(|&p| p != id);
        st.pick_purposes.remove(&id);
        st.answered.insert(id, picked);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_queues_action_and_completes_with_paths() {
        let files = IosFiles::new();
        assert!(files.can_pick());
        assert!(!files.browser_saves());

        let req = PickRequest::open("document").filter("Documents", &["docx", "txt"]);
        let id = files.pick(req);
        assert!(files.pick_pending());

        let action = files.pop_action().expect("queued pick action");
        assert_eq!(action["action"], "file.pick");
        assert_eq!(action["pick_id"], id.0);
        assert_eq!(action["kind"], "open");

        files.complete_pick(id.0, vec![PathBuf::from("/tmp/example.docx")]);
        assert!(!files.pick_pending());
        assert_eq!(files.take_answer(id), Some(Picked::Paths(vec![PathBuf::from("/tmp/example.docx")])));
        assert_eq!(files.last_folder("document"), Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn write_writes_directly_without_sibling_temp_files() {
        let files = IosFiles::new();
        let dir = std::env::temp_dir().join(format!("wc-ios-files-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("doc.docx");

        files.write(&path, b"PK-mock-docx", WriteIntent::Save).expect("direct write succeeds");
        assert_eq!(files.read(&path).expect("read back"), b"PK-mock-docx");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
