//! Files that hold secrets: created readable by their owner only, and refused when the group or
//! others can read them.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, bail};

/// Write `contents` to a new file readable only by its owner. Fails when the file exists.
pub(crate) fn write_secret(path: &Path, contents: &str) -> anyhow::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Refuse a secret file that the group or others can read.
pub(crate) fn check_private(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .with_context(|| format!("cannot read {}", path.display()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            bail!(
                "{} is readable by group or others (mode {:o}); run chmod 600 on it",
                path.display(),
                mode & 0o777
            );
        }
    }
    Ok(())
}
