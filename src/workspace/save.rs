use std::{
    collections::hash_map::RandomState,
    fs::{self, File},
    hash::{BuildHasher, Hasher},
    io::{self, Read},
    path::Path,
    sync::OnceLock,
};

use super::WorkspaceError;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct FileBaseline {
    bytes: u64,
    hash: u64,
}

#[derive(Clone, Copy)]
pub(super) enum ExpectedFile {
    Absent,
    OverwriteExisting,
    Unchanged(FileBaseline),
}

pub(super) fn create_staged(path: &Path, _expected: ExpectedFile) -> io::Result<File> {
    let mut options = File::options();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if !matches!(_expected, ExpectedFile::Absent) {
        // An existing document can be private even in a publicly readable directory.
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }
    options.open(path)
}

#[cfg(not(windows))]
pub(super) fn same_path(saved: &Path, selected: &Path) -> io::Result<bool> {
    Ok(saved == selected)
}

#[cfg(windows)]
pub(super) fn same_path(saved: &Path, selected: &Path) -> io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;

    if saved == selected {
        return Ok(true);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CompareStringOrdinal(
            a: *const u16,
            na: i32,
            b: *const u16,
            nb: i32,
            ignore_case: i32,
        ) -> i32;
    }
    let a: Vec<_> = saved.as_os_str().encode_wide().collect();
    let b: Vec<_> = selected.as_os_str().encode_wide().collect();
    // SAFETY: both counted UTF-16 buffers live throughout the call.
    match unsafe { CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) }
    {
        0 => return Err(io::Error::last_os_error()),
        2 => {}
        _ => return Ok(false),
    }
    // A case-sensitive directory can contain both spellings as distinct entries.
    let original = match fs::canonicalize(saved) {
        Ok(path) => path,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e),
    };
    match fs::canonicalize(selected) {
        Ok(path) => Ok(original == path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

const BLOCK: usize = 8192;

fn hasher() -> impl Hasher {
    static SEED: OnceLock<RandomState> = OnceLock::new();
    SEED.get_or_init(RandomState::new).build_hasher()
}

impl FileBaseline {
    pub(super) fn of_bytes(bytes: &[u8]) -> Self {
        let mut hash = hasher();
        for block in bytes.chunks(BLOCK) {
            hash.write(block);
        }
        Self {
            bytes: bytes.len() as u64,
            hash: hash.finish(),
        }
    }

    pub(super) fn of_file(mut file: &File) -> io::Result<Self> {
        let bytes = file.metadata()?.len();
        let mut hash = hasher();
        let mut buffer = [0; BLOCK];
        let mut remaining = bytes;
        while remaining > 0 {
            let count = remaining.min(BLOCK as u64) as usize;
            file.read_exact(&mut buffer[..count])?;
            hash.write(&buffer[..count]);
            remaining -= count as u64;
        }
        Ok(Self {
            bytes,
            hash: hash.finish(),
        })
    }

    fn verify(self, file: &File, path: &Path) -> Result<(), WorkspaceError> {
        if file.metadata()?.len() != self.bytes || Self::of_file(file)? != self {
            return Err(WorkspaceError::SaveConflict(path.into()));
        }
        Ok(())
    }
}

// The name stays stable across atomic replacement. The last handle (including
// one closed by process exit) releases it without leaving a file in the project.
#[cfg(windows)]
pub(super) fn replace(
    staged: &Path,
    path: &Path,
    expected: ExpectedFile,
) -> Result<(), WorkspaceError> {
    use std::{
        ffi::c_void,
        os::windows::{
            ffi::OsStrExt,
            io::{FromRawHandle, OwnedHandle},
        },
        ptr,
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attributes: *const c_void, owner: i32, name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
        fn MoveFileExW(source: *const u16, target: *const u16, flags: u32) -> i32;
    }
    if matches!(expected, ExpectedFile::Absent) {
        let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<_>>();
        let source = wide(staged);
        let target = wide(path);
        // SAFETY: both absolute paths are NUL-terminated. Flags 0 forbids replacement.
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } == 0 {
            return Err(publish_error(io::Error::last_os_error(), path));
        }
        return Ok(());
    }
    // FNV-1a over the canonical UTF-16 path gives all processes the same short name.
    let key = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .fold(0xcbf29ce484222325_u64, |key, c| {
            (key ^ u64::from(c)).wrapping_mul(0x100000001b3)
        });
    let name: Vec<u16> = format!("Global\\Reliner.Save.{key:016x}\0")
        .encode_utf16()
        .collect();
    // SAFETY: name is NUL-terminated. CreateMutexW returns either NULL or an owned handle.
    let (handle, error) = unsafe {
        let handle = CreateMutexW(ptr::null(), 0, name.as_ptr());
        (handle, GetLastError())
    };
    if handle.is_null() {
        return Err(io::Error::from_raw_os_error(error as i32).into());
    }
    // SAFETY: this call owns one reference to the non-null kernel handle.
    let _guard = unsafe { OwnedHandle::from_raw_handle(handle) };
    const ERROR_ALREADY_EXISTS: u32 = 183;
    if error == ERROR_ALREADY_EXISTS {
        return Err(WorkspaceError::SaveConflict(path.into()));
    }
    match expected {
        ExpectedFile::Unchanged(stamp) => {
            stamp.verify(
                &File::open(path).map_err(|e| existing_error(e, path))?,
                path,
            )?;
        }
        ExpectedFile::OverwriteExisting => {
            fs::metadata(path).map_err(|e| existing_error(e, path))?;
        }
        ExpectedFile::Absent => unreachable!(),
    }
    fs::rename(staged, path)?;
    Ok(())
}

fn existing_error(e: io::Error, path: &Path) -> WorkspaceError {
    match e.kind() {
        io::ErrorKind::NotFound => WorkspaceError::SaveConflict(path.into()),
        _ => e.into(),
    }
}

fn publish_error(e: io::Error, path: &Path) -> WorkspaceError {
    if e.kind() == io::ErrorKind::AlreadyExists {
        WorkspaceError::SaveConflict(path.into())
    } else {
        e.into()
    }
}

#[cfg(unix)]
pub(super) fn replace(
    staged: &Path,
    path: &Path,
    expected: ExpectedFile,
) -> Result<(), WorkspaceError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let conflict = || WorkspaceError::SaveConflict(path.into());
    if matches!(expected, ExpectedFile::Absent) {
        fs::hard_link(staged, path).map_err(|e| publish_error(e, path))?;
        // The target is already committed. Cleanup cannot change the save receipt.
        if let Err(e) = fs::remove_file(staged) {
            use std::io::Write;
            let _ = writeln!(
                io::stderr().lock(),
                "已保存 {}，清理临时文件 {} 失败：{e}",
                path.display(),
                staged.display()
            );
        }
        return Ok(());
    }
    let meta = fs::metadata(path).map_err(|e| existing_error(e, path))?;
    if !meta.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "保存目标须为普通文件。").into());
    }
    // Directory rename permission alone must not grant write access to a document.
    let file = File::options()
        .read(matches!(expected, ExpectedFile::Unchanged(_)))
        .write(true)
        .open(path)
        .map_err(|e| existing_error(e, path))?;
    file.try_lock().map_err(|e| match e {
        fs::TryLockError::WouldBlock => conflict(),
        fs::TryLockError::Error(e) => e.into(),
    })?;
    // A previous writer may have replaced this inode between open and lock.
    let locked = file.metadata()?;
    if locked.permissions().readonly() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "保存目标为只读文件。").into());
    }
    let current = fs::metadata(path)?;
    if (locked.dev(), locked.ino()) != (current.dev(), current.ino()) {
        return Err(conflict());
    }
    if let ExpectedFile::Unchanged(expected) = expected {
        expected.verify(&file, path)?;
    }
    // Keep ordinary access bits, never restore set-ID privileges on edited contents.
    fs::set_permissions(staged, fs::Permissions::from_mode(locked.mode() & 0o777))?;
    fs::rename(staged, path)?;
    Ok(())
}
