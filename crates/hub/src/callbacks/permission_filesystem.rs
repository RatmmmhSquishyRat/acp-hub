use super::*;

const MAX_READ_TEXT_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_WRITE_TEXT_FILE_BYTES: usize = 16 * 1024 * 1024;

impl HubCtx {
    // ---- permission --------------------------------------------------------

    pub fn handle_permission(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, HubError> {
        self.connection(agent_id, connection_id)?;
        let policy = self
            .binding(agent_id, req.session_id.to_string().as_str())?
            .permission_policy;
        let outcome = match policy {
            PermissionPolicy::AutoAllow => first_option(req, true),
            PermissionPolicy::AutoCancel => RequestPermissionOutcome::Cancelled,
            PermissionPolicy::Reject => first_option(req, false),
        };
        Ok(RequestPermissionResponse::new(outcome))
    }

    // ---- fs ----------------------------------------------------------------

    pub fn handle_read_text_file(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &ReadTextFileRequest,
    ) -> Result<ReadTextFileResponse, HubError> {
        let binding = self.binding(agent_id, req.session_id.to_string().as_str())?;
        let advertised = self
            .connection(agent_id, connection_id)?
            .config
            .client_capabilities
            .fs
            .read_text_file;
        if !advertised || !binding.fs.read_text_file {
            return Err(HubError::other("fs/read_text_file not enabled"));
        }
        let path = resolve(&req.path, &binding.fs.allowed_roots, &binding.cwd)?;
        let mut file = fs::File::open(&path)
            .map_err(|e| HubError::other(format!("open {}: {e}", path.display())))?;
        let metadata = file.metadata()?;
        if metadata.len() > MAX_READ_TEXT_FILE_BYTES {
            return Err(HubError::other(format!(
                "file exceeds the {MAX_READ_TEXT_FILE_BYTES}-byte callback read limit"
            )));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_READ_TEXT_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| HubError::other(format!("read {}: {e}", path.display())))?;
        if bytes.len() as u64 > MAX_READ_TEXT_FILE_BYTES {
            return Err(HubError::other(format!(
                "file exceeds the {MAX_READ_TEXT_FILE_BYTES}-byte callback read limit"
            )));
        }
        let text = String::from_utf8(bytes)
            .map_err(|e| HubError::other(format!("read {}: {e}", path.display())))?;
        Ok(ReadTextFileResponse::new(slice_lines(
            &text, req.line, req.limit,
        )))
    }

    pub fn handle_write_text_file(
        &self,
        agent_id: &str,
        connection_id: &str,
        req: &WriteTextFileRequest,
    ) -> Result<WriteTextFileResponse, HubError> {
        let binding = self.binding(agent_id, req.session_id.to_string().as_str())?;
        let advertised = self
            .connection(agent_id, connection_id)?
            .config
            .client_capabilities
            .fs
            .write_text_file;
        if !advertised || !binding.fs.write_text_file {
            return Err(HubError::other("fs/write_text_file not enabled"));
        }
        if req.content.len() > MAX_WRITE_TEXT_FILE_BYTES {
            return Err(HubError::other(format!(
                "write content exceeds the {MAX_WRITE_TEXT_FILE_BYTES}-byte callback limit"
            )));
        }
        let path = resolve(&req.path, &binding.fs.allowed_roots, &binding.cwd)?;
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        write_text_no_follow(&path, req.content.as_bytes())?;
        Ok(WriteTextFileResponse::new())
    }
}

fn first_option(req: &RequestPermissionRequest, allow: bool) -> RequestPermissionOutcome {
    let desired: &[PermissionOptionKind] = if allow {
        &[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ]
    } else {
        &[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ]
    };
    for opt in &req.options {
        if desired.contains(&opt.kind) {
            return RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                opt.option_id.clone(),
            ));
        }
    }
    RequestPermissionOutcome::Cancelled
}

pub(super) fn resolve(path: &Path, roots: &[PathBuf], cwd: &Path) -> Result<PathBuf, HubError> {
    let r = if path.is_absolute() {
        path.into()
    } else {
        cwd.join(path)
    };
    let c = match r.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            // Target doesn't exist yet (e.g. writing a new file): canonicalize the
            // existing parent and re-attach the leaf component, so the allowed-roots
            // check below still confines the write.
            //
            // If the leaf already exists but cannot be canonicalized, it may be a
            // dangling symlink. Treat it as an invalid target instead of re-attaching
            // it: a later write would follow that symlink outside the allowed root.
            if std::fs::symlink_metadata(&r).is_ok() {
                return Err(HubError::other(format!(
                    "resolve {}: existing target could not be canonicalized",
                    r.display()
                )));
            }
            let parent = r.parent().unwrap_or_else(|| Path::new(""));
            let leaf = r
                .file_name()
                .ok_or_else(|| HubError::other(format!("invalid path: {}", r.display())))?;
            let pc = parent
                .canonicalize()
                .map_err(|e| HubError::other(format!("resolve {}: {e}", r.display())))?;
            pc.join(leaf)
        }
    };
    let allowed: Vec<PathBuf> = if roots.is_empty() {
        vec![cwd.canonicalize().unwrap_or_else(|_| cwd.into())]
    } else {
        roots
            .iter()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
            .collect()
    };
    for root in &allowed {
        if c.starts_with(root) {
            return Ok(c);
        }
    }
    Err(HubError::other(format!(
        "{} outside allowed roots",
        c.display()
    )))
}

pub(super) fn write_text_no_follow(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the reparse point itself instead of following a final-component
        // symlink/junction that appeared between resolve() and open().
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path)?;
    if file.metadata()?.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to write through a symlink",
        ));
    }
    file.write_all(content)
}

fn slice_lines(text: &str, line: Option<u32>, limit: Option<u32>) -> String {
    match (line, limit) {
        (None, None) | (Some(0), _) => text.into(),
        _ => {
            let s = line.unwrap_or(1) as usize;
            let n = limit.map(|l| l as usize).unwrap_or(usize::MAX);
            text.lines()
                .skip(s.saturating_sub(1))
                .take(n)
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}
