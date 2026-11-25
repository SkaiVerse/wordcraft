//! Native iPadOS `Files` service bridging WordCraft's non-blocking Open/Save/Export requests
//! to UIKit `UIDocumentPickerViewController` in sandbox-safe copy/export mode (`browser_saves = true`
//! with local `Recents` cache fallback for File > Open Recent).

use craft::host::{FileError, Files, PickId, PickKind, PickRequest, Picked, WriteIntent};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct FilesState {
    next_id: u64,
    pending_picks: Vec<PickId>,
    pick_purposes: HashMap<PickId, String>,
    pending_open_contents: HashMap<PickId, Vec<(String, Vec<u8>)>>,
    answered: HashMap<PickId, Picked>,
    staging_paths: HashMap<PickId, PathBuf>,
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

    fn caches_dir(sub: &str) -> PathBuf {
        let base = std::env::var_os("HOME").map(PathBuf::from).map(|h| h.join("Library/Caches")).unwrap_or_else(std::env::temp_dir);
        let dir = base.join(sub);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn cache_recent(name: &str, bytes: &[u8]) {
        if let Some(fname) = Path::new(name).file_name() {
            let dest = Self::caches_dir("WordCraftRecents").join(fname);
            let _ = std::fs::write(dest, bytes);
        }
    }

    /// Pops the next queued UIKit file-picker request as JSON, if any.
    pub fn pop_action(&self) -> Option<Value> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).outgoing_actions.pop_front()
    }

    /// Appends one opened file's `(name, bytes)` from UIKit `UIDocumentPickerViewController(asCopy: true)`.
    pub fn push_opened_file_bytes(&self, pick_id: u64, name: &str, bytes: Vec<u8>) {
        Self::cache_recent(name, &bytes);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending_open_contents.entry(PickId(pick_id)).or_default().push((name.to_string(), bytes));
    }

    /// Finalizes a pending `PickKind::Open` / `OpenMany` ticket after all picked files have been pushed
    /// (empty = cancelled).
    pub fn finish_open_pick(&self, pick_id: u64) {
        let id = PickId(pick_id);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending_picks.retain(|&p| p != id);
        let purpose = st.pick_purposes.remove(&id).unwrap_or_default();
        let files = st.pending_open_contents.remove(&id).unwrap_or_default();

        if files.is_empty() {
            st.answered.insert(id, Picked::Cancelled);
            return;
        }

        // Only Bibliography `Sources.xml` requires a path instead of `Picked::Contents`.
        if purpose == "sources" {
            let inbox = Self::caches_dir("WordCraftInbox");
            let mut paths = Vec::with_capacity(files.len());
            for (name, bytes) in files {
                let safe_name = Path::new(&name).file_name().unwrap_or_else(|| std::ffi::OsStr::new("file"));
                let dest = inbox.join(safe_name);
                if let Err(err) = std::fs::write(&dest, &bytes) {
                    st.answered.insert(id, Picked::Failed(err.to_string()));
                    return;
                }
                paths.push(dest);
            }
            st.answered.insert(id, Picked::Paths(paths));
        } else {
            st.answered.insert(id, Picked::Contents(files));
        }
    }

    /// Completes an active `save` export sheet (`saved == true` -> `Picked::Downloaded`, else `Cancelled`).
    pub fn complete_save_pick(&self, pick_id: u64, saved: bool, name: Option<&str>) {
        let id = PickId(pick_id);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending_picks.retain(|&p| p != id);
        st.pick_purposes.remove(&id);
        if let Some(staging) = st.staging_paths.remove(&id) {
            let _ = std::fs::remove_file(staging);
        }
        let picked = if saved { Picked::Downloaded(name.unwrap_or("Document.docx").to_string()) } else { Picked::Cancelled };
        st.answered.insert(id, picked);
    }

    /// Completes an active file picker ticket with path(s) (or cancels when empty).
    pub fn complete_pick(&self, pick_id: u64, paths: Vec<PathBuf>) {
        let id = PickId(pick_id);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending_picks.retain(|&p| p != id);
        let purpose = st.pick_purposes.remove(&id).unwrap_or_default();
        st.pending_open_contents.remove(&id);
        if let Some(staging) = st.staging_paths.remove(&id) {
            let _ = std::fs::remove_file(staging);
        }

        let picked = if paths.is_empty() {
            Picked::Cancelled
        } else {
            if let Some(parent) = paths.first().and_then(|p| p.parent())
                && !purpose.is_empty()
            {
                st.last_folders.insert(purpose, parent.to_path_buf());
            }
            Picked::Paths(paths)
        };
        st.answered.insert(id, picked);
    }

    /// Fails an active picker ticket with an error reason.
    pub fn fail_pick(&self, pick_id: u64, reason: String) {
        let id = PickId(pick_id);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending_picks.retain(|&p| p != id);
        st.pick_purposes.remove(&id);
        st.pending_open_contents.remove(&id);
        if let Some(staging) = st.staging_paths.remove(&id) {
            let _ = std::fs::remove_file(staging);
        }
        st.answered.insert(id, Picked::Failed(reason));
    }
}

fn map_io_error(err: std::io::Error, path: &Path) -> FileError {
    match err.kind() {
        std::io::ErrorKind::NotFound => FileError::NotFound(path.to_path_buf()),
        std::io::ErrorKind::PermissionDenied => FileError::PermissionDenied(path.to_path_buf()),
        _ => FileError::Other(err.to_string()),
    }
}

impl Files for IosFiles {
    fn pick(&self, request: PickRequest) -> PickId {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.next_id += 1;
        let id = PickId(st.next_id);
        st.pending_picks.push(id);
        st.pick_purposes.insert(id, request.purpose.clone());

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

    fn take_answer(&self, id: PickId) -> Option<Picked> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).answered.remove(&id)
    }

    fn forget(&self, id: PickId) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending_picks.retain(|&p| p != id);
        st.pick_purposes.remove(&id);
        st.pending_open_contents.remove(&id);
        st.answered.remove(&id);
        if let Some(staging) = st.staging_paths.remove(&id) {
            let _ = std::fs::remove_file(staging);
        }
    }

    fn pick_pending(&self) -> bool {
        !self.state.lock().unwrap_or_else(|e| e.into_inner()).pending_picks.is_empty()
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>, FileError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(bytes),
            Err(first_err) => {
                if let Some(fname) = path.file_name() {
                    let cached = Self::caches_dir("WordCraftRecents").join(fname);
                    if let Ok(bytes) = std::fs::read(&cached) {
                        return Ok(bytes);
                    }
                }
                Err(map_io_error(first_err, path))
            }
        }
    }

    fn write(&self, path: &Path, bytes: &[u8], _intent: WriteIntent) -> Result<bool, FileError> {
        std::fs::write(path, bytes).map_err(|e| map_io_error(e, path))?;
        Ok(false)
    }

    fn exists(&self, path: &Path) -> bool {
        if path.exists() {
            return true;
        }
        path.file_name().is_some_and(|fname| Self::caches_dir("WordCraftRecents").join(fname).exists())
    }

    fn last_folder(&self, purpose: &str) -> Option<PathBuf> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).last_folders.get(purpose).cloned()
    }

    fn save(&self, request: PickRequest, bytes: Vec<u8>) -> PickId {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.next_id += 1;
        let id = PickId(st.next_id);
        st.pending_picks.push(id);
        st.pick_purposes.insert(id, request.purpose.clone());

        let file_name = request
            .file_name
            .clone()
            .unwrap_or_else(|| if let Some(ext) = &request.suggested_extension { format!("Document.{ext}") } else { "Document.docx".to_string() });
        let safe_name = Path::new(&file_name).file_name().and_then(|n| n.to_str()).unwrap_or("Document.docx").to_string();

        Self::cache_recent(&safe_name, &bytes);

        let export_dir = Self::caches_dir("WordCraftExport").join(format!("pick_{}", id.0));
        let _ = std::fs::create_dir_all(&export_dir);
        let staging_path = export_dir.join(&safe_name);
        if let Err(err) = std::fs::write(&staging_path, &bytes) {
            st.pending_picks.retain(|&p| p != id);
            st.pick_purposes.remove(&id);
            st.answered.insert(id, Picked::Failed(err.to_string()));
            return id;
        }

        st.staging_paths.insert(id, staging_path.clone());
        st.outgoing_actions.push_back(json!({
            "action": "file.save_bytes",
            "pick_id": id.0,
            "file_name": safe_name,
            "staging_path": staging_path.to_string_lossy(),
        }));
        id
    }

    fn can_pick(&self) -> bool {
        true
    }

    fn browser_saves(&self) -> bool {
        true
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
    fn save_bytes_writes_staging_file_and_completes_as_downloaded() {
        let files = IosFiles::new();
        assert!(files.can_pick());
        assert!(files.browser_saves());

        let req = PickRequest::save("document").file_name("report.docx");
        let id = files.save(req, b"PK-mock-docx".to_vec());
        assert!(files.pick_pending());

        let action = files.pop_action().expect("queued save_bytes action");
        assert_eq!(action["action"], "file.save_bytes");
        assert_eq!(action["pick_id"], id.0);
        assert_eq!(action["file_name"], "report.docx");
        let staging = PathBuf::from(action["staging_path"].as_str().unwrap());
        assert_eq!(std::fs::read(&staging).unwrap(), b"PK-mock-docx");

        files.complete_save_pick(id.0, true, Some("report.docx"));
        assert!(!files.pick_pending());
        assert_eq!(files.take_answer(id), Some(Picked::Downloaded("report.docx".to_string())));
        assert!(!staging.exists());
    }

    #[test]
    fn open_bytes_pushes_contents_and_caches_recent() {
        let files = IosFiles::new();
        let req = PickRequest::open("document");
        let id = files.pick(req);

        let action = files.pop_action().expect("queued file.pick");
        assert_eq!(action["action"], "file.pick");

        files.push_opened_file_bytes(id.0, "notes.docx", b"hello-docx".to_vec());
        files.finish_open_pick(id.0);

        assert_eq!(files.take_answer(id), Some(Picked::Contents(vec![("notes.docx".to_string(), b"hello-docx".to_vec())])));
        assert!(files.exists(Path::new("/nonexistent/notes.docx")));
        assert_eq!(files.read(Path::new("/nonexistent/notes.docx")).unwrap(), b"hello-docx");
    }
}
