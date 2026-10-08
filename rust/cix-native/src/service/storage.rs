//! Directory-handle confinement for server-owned state. No raw tenant paths.
use super::error::{Problem, Result};
use std::{
    fs::File,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct Directory {
    #[cfg(unix)]
    file: Arc<File>,
    path: PathBuf,
}

pub(crate) fn tenant_components(tenant: &str) -> Result<Vec<String>> {
    if !super::config::safe_identity(tenant) {
        return Err(Problem::invalid("tenant"));
    }
    let mut encoded = String::with_capacity(tenant.len() * 2);
    use std::fmt::Write;
    for byte in tenant.as_bytes() {
        write!(&mut encoded, "{byte:02x}").map_err(|_| Problem::internal())?;
    }
    let mut result = vec!["v1".to_owned()];
    result.extend(
        encoded
            .as_bytes()
            .chunks(128)
            .map(|chunk| String::from_utf8(chunk.to_vec()).expect("hex ASCII")),
    );
    Ok(result)
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        ffi::{CStr, CString, OsString},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStringExt, fs::MetadataExt},
        },
    };

    fn component(name: &str) -> Result<CString> {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return Err(Problem::invalid("storage_component"));
        }
        CString::new(name).map_err(|_| Problem::invalid("storage_component"))
    }
    fn checked_file(fd: libc::c_int) -> Result<File> {
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // Every caller transfers a freshly opened descriptor exactly once.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    fn directory_file(parent: &File, name: &CStr, create: bool) -> Result<File> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let mut fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0
            && create
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
        {
            let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
            if result != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error().into());
            }
            fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        }
        checked_file(fd)
    }
    fn private(file: &File) -> Result<()> {
        let meta = file.metadata()?;
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(Problem::new("insecure_state_directory", 400));
        }
        Ok(())
    }
    fn regular(file: &File) -> Result<()> {
        let meta = file.metadata()?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.nlink() != 1
            || meta.mode() & 0o022 != 0
        {
            return Err(Problem::new("object_integrity", 422));
        }
        Ok(())
    }
    impl Directory {
        pub(crate) fn open_private(path: &Path) -> Result<Self> {
            if !path.is_absolute() {
                return Err(Problem::invalid("state_directory"));
            }
            let root = CString::new("/").expect("literal");
            let mut file = checked_file(unsafe {
                libc::open(
                    root.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            })?;
            let mut parts = 0;
            for part in path.components() {
                match part {
                    Component::RootDir => {}
                    Component::Normal(value) => {
                        let name = CString::new(value.as_encoded_bytes())
                            .map_err(|_| Problem::invalid("state_directory"))?;
                        file = directory_file(&file, &name, true)?;
                        parts += 1;
                    }
                    _ => return Err(Problem::invalid("state_directory")),
                }
            }
            if parts == 0 {
                return Err(Problem::invalid("state_directory"));
            }
            private(&file)?;
            Ok(Self {
                file: Arc::new(file),
                path: path.into(),
            })
        }
        pub(crate) fn child(&self, name: &str, create: bool) -> Result<Self> {
            let file = directory_file(&self.file, &component(name)?, create)?;
            private(&file)?;
            Ok(Self {
                file: Arc::new(file),
                path: self.path.join(name),
            })
        }
        pub(crate) fn tenant(&self, name: &str, create: bool) -> Result<Self> {
            let mut directory = self.clone();
            for part in tenant_components(name)? {
                directory = directory.child(&part, create)?;
            }
            Ok(directory)
        }
        pub(crate) fn create_file(&self, name: &str) -> Result<File> {
            let name = component(name)?;
            let file = checked_file(unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            })?;
            regular(&file)?;
            Ok(file)
        }
        pub(crate) fn open_file(&self, name: &str) -> Result<File> {
            let name = component(name)?;
            let file = checked_file(unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                )
            })?;
            regular(&file)?;
            Ok(file)
        }
        /// Stable parent-owned descriptor path for the isolated same-UID worker.
        /// Unsupported platforms fail closed until their equivalent is qualified.
        pub(crate) fn access_path(&self) -> Result<PathBuf> {
            #[cfg(target_os = "linux")]
            {
                Ok(PathBuf::from(format!(
                    "/proc/{}/fd/{}",
                    std::process::id(),
                    self.file.as_raw_fd()
                )))
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(Problem::new("confined_storage_unavailable", 503))
            }
        }
        pub(crate) fn sqlite_probe_url(&self) -> Result<Option<String>> {
            let mut exists = false;
            for name in [
                "metadata.sqlite3",
                "metadata.sqlite3-wal",
                "metadata.sqlite3-shm",
                "metadata.sqlite3-journal",
            ] {
                let value = component(name)?;
                let fd = unsafe {
                    libc::openat(
                        self.file.as_raw_fd(),
                        value.as_ptr(),
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                    )
                };
                if fd >= 0 {
                    let file = checked_file(fd)?;
                    regular(&file)?;
                    let length = file.metadata()?.len();
                    if name == "metadata.sqlite3" {
                        exists = length != 0;
                    }
                    // An immutable read cannot see uncheckpointed WAL/journal data.
                    // Fail before opening SQLite; recovery requires explicit ownership.
                    else if length != 0 {
                        return Err(Problem::new("state_recovery_required", 503));
                    }
                } else if std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
                    return Err(Problem::new("object_integrity", 422));
                }
            }
            Ok(if exists {
                Some(format!(
                    "sqlite://{}/metadata.sqlite3?mode=ro&immutable=true",
                    self.access_path()?.display()
                ))
            } else {
                None
            })
        }
        pub(crate) fn sqlite_url(&self) -> Result<String> {
            let name = component("metadata.sqlite3")?;
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                )
            };
            if fd >= 0 {
                regular(&checked_file(fd)?)?;
            } else if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                let file = self.create_file("metadata.sqlite3")?;
                file.sync_all()?;
                self.sync()?;
            } else {
                return Err(Problem::new("object_integrity", 422));
            }
            Ok(format!(
                "sqlite://{}/metadata.sqlite3?mode=rw",
                self.access_path()?.display()
            ))
        }
        pub(crate) fn sync(&self) -> Result<()> {
            self.file.sync_all().map_err(Problem::from)
        }
        #[cfg(test)]
        pub(crate) fn path(&self) -> &Path {
            &self.path
        }
        pub(crate) fn unlink_file(&self, name: &str) -> Result<()> {
            let name = component(name)?;
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            self.sync()
        }
        /// Cleanup only the private worker subtree, without following its links.
        pub(crate) fn remove_child(&self, name: &str) -> Result<()> {
            let child = self.child(name, false)?;
            child.clear(0)?;
            let component = component(name)?;
            if unsafe {
                libc::unlinkat(
                    self.file.as_raw_fd(),
                    component.as_ptr(),
                    libc::AT_REMOVEDIR,
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            self.sync()
        }
        fn names(&self) -> Result<Vec<OsString>> {
            let dot = CString::new(".").expect("literal");
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    dot.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let directory = unsafe { libc::fdopendir(fd) };
            if directory.is_null() {
                unsafe {
                    libc::close(fd);
                }
                return Err(std::io::Error::last_os_error().into());
            }
            struct Scan(*mut libc::DIR);
            impl Drop for Scan {
                fn drop(&mut self) {
                    unsafe {
                        libc::closedir(self.0);
                    }
                }
            }
            let scan = Scan(directory);
            let mut result = Vec::new();
            loop {
                let entry = unsafe { libc::readdir(scan.0) };
                if entry.is_null() {
                    break;
                }
                let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                if name == b"." || name == b".." {
                    continue;
                }
                if result.len() >= 10000 {
                    return Err(Problem::limit("worker_directory_entries"));
                }
                result.push(OsString::from_vec(name.to_vec()));
            }
            Ok(result)
        }
        fn clear(&self, depth: usize) -> Result<()> {
            if depth > 32 {
                return Err(Problem::limit("worker_directory_depth"));
            }
            for name in self.names()? {
                let value = CString::new(name.as_encoded_bytes())
                    .map_err(|_| Problem::invalid("storage_component"))?;
                let mut meta = std::mem::MaybeUninit::<libc::stat>::uninit();
                if unsafe {
                    libc::fstatat(
                        self.file.as_raw_fd(),
                        value.as_ptr(),
                        meta.as_mut_ptr(),
                        libc::AT_SYMLINK_NOFOLLOW,
                    )
                } != 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                let meta = unsafe { meta.assume_init() };
                if meta.st_uid != unsafe { libc::geteuid() } {
                    return Err(Problem::new("object_integrity", 422));
                }
                if meta.st_mode & libc::S_IFMT == libc::S_IFDIR {
                    let file = directory_file(&self.file, &value, false)?;
                    private(&file)?;
                    let directory = Self {
                        file: Arc::new(file),
                        path: self.path.join(&name),
                    };
                    directory.clear(depth + 1)?;
                    if unsafe {
                        libc::unlinkat(self.file.as_raw_fd(), value.as_ptr(), libc::AT_REMOVEDIR)
                    } != 0
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                } else if unsafe { libc::unlinkat(self.file.as_raw_fd(), value.as_ptr(), 0) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            Ok(())
        }
    }
}

#[cfg(not(unix))]
impl Directory {
    fn unsupported<T>() -> Result<T> {
        Err(Problem::new("confined_storage_unavailable", 503))
    }
    pub(crate) fn open_private(_: &Path) -> Result<Self> {
        Self::unsupported()
    }
    pub(crate) fn child(&self, _: &str, _: bool) -> Result<Self> {
        Self::unsupported()
    }
    pub(crate) fn tenant(&self, _: &str, _: bool) -> Result<Self> {
        Self::unsupported()
    }
    pub(crate) fn create_file(&self, _: &str) -> Result<File> {
        Self::unsupported()
    }
    pub(crate) fn open_file(&self, _: &str) -> Result<File> {
        Self::unsupported()
    }
    pub(crate) fn access_path(&self) -> Result<PathBuf> {
        Self::unsupported()
    }
    pub(crate) fn sqlite_probe_url(&self) -> Result<Option<String>> {
        Self::unsupported()
    }
    pub(crate) fn sqlite_url(&self) -> Result<String> {
        Self::unsupported()
    }
    pub(crate) fn sync(&self) -> Result<()> {
        Self::unsupported()
    }
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn unlink_file(&self, _: &str) -> Result<()> {
        Self::unsupported()
    }
    pub(crate) fn remove_child(&self, _: &str) -> Result<()> {
        Self::unsupported()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::fs::symlink,
    };
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("cix-storage-{}", super::super::contracts::new_id())),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn tenant_encoding_is_reversible_bounded_and_distinct() {
        for tenant in [
            "../a/b".to_owned(),
            "日本語/é".to_owned(),
            "é".repeat(128),
            "a".repeat(256),
        ] {
            let parts = tenant_components(&tenant).unwrap();
            assert_eq!(parts[0], "v1");
            assert!(parts.len() <= 5);
            assert!(parts[1..].iter().all(|p| p.len() <= 128));
            let joined = parts[1..].concat();
            let decoded = joined
                .as_bytes()
                .chunks_exact(2)
                .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(decoded, tenant.as_bytes());
        }
        assert_ne!(
            tenant_components("a/b").unwrap(),
            tenant_components("a\\b").unwrap()
        );
        assert!(tenant_components(&"a".repeat(257)).is_err());
    }
    #[test]
    fn handles_refuse_symlinks_and_hardlinks_and_survive_parent_rename() {
        let f = Fixture::new();
        let root = Directory::open_private(&f.0).unwrap();
        let child = root.child("child", true).unwrap();
        let mut output = child.create_file("object").unwrap();
        output.write_all(b"exact input").unwrap();
        output.sync_all().unwrap();
        drop(output);
        symlink("object", child.path().join("link")).unwrap();
        assert!(child.open_file("link").is_err());
        std::fs::hard_link(child.path().join("object"), child.path().join("hard")).unwrap();
        assert!(child.open_file("object").is_err());
        std::fs::remove_file(child.path().join("hard")).unwrap();
        std::fs::rename(child.path(), root.path().join("renamed")).unwrap();
        symlink("renamed", root.path().join("child")).unwrap();
        assert!(root.child("child", false).is_err());
        let mut bytes = Vec::new();
        child
            .open_file("object")
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"exact input");
        root.remove_child("renamed").unwrap();
        assert!(root
            .path()
            .join("child")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }
    #[test]
    fn sqlite_probe_rejects_recovery_sidecars_without_creating_database() {
        let f = Fixture::new();
        let root = Directory::open_private(&f.0).unwrap();
        let mut wal = root.create_file("metadata.sqlite3-wal").unwrap();
        wal.write_all(b"uncheckpointed").unwrap();
        drop(wal);
        assert_eq!(
            root.sqlite_probe_url().unwrap_err().code,
            "state_recovery_required"
        );
        assert!(!f.0.join("metadata.sqlite3").exists());
    }
}
