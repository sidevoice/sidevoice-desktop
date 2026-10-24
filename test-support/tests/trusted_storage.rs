#![cfg(unix)]

// The former local trusted implementation's regressions now exercise the shared owner directly.
mod tests {
    use sidevoice_platform::fs::{Access, Directory, Files, Fs, HeldDirectory};
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;
    use std::{io::ErrorKind, path::Path};

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("svtr").tempdir_in("/tmp").unwrap()
    }

    #[test]
    fn ancestry_others_can_write_is_refused_unless_sticky() {
        let tmp = tmp();
        let shared = tmp.path().join("shared");
        let data = shared.join("sidevoice");
        std::fs::create_dir_all(&data).unwrap();
        chmod(&shared, 0o755);
        assert!(HeldDirectory::open(&data, Access::NotWritableByOthers).is_ok());
        chmod(&shared, 0o777);
        let error = HeldDirectory::open(&data, Access::NotWritableByOthers).err().expect("a writable parent passed");
        assert_eq!(error.kind(), ErrorKind::PermissionDenied);
        chmod(&shared, 0o775);
        assert!(HeldDirectory::open(&data, Access::NotWritableByOthers).is_err(), "group-writable");
        chmod(&shared, 0o1777);
        assert!(HeldDirectory::open(&data, Access::NotWritableByOthers).is_ok(), "sticky, as /tmp");
        assert_eq!(
            HeldDirectory::open(&tmp.path().join("none"), Access::Private).err().unwrap().kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn a_directory_replaced_after_it_was_opened_is_not_read() {
        let tmp = tmp();
        let data = tmp.path().join("d");
        std::fs::create_dir(&data).unwrap();
        chmod(&data, 0o700);
        std::fs::write(data.join("install.json"), b"genuine").unwrap();
        let dir = HeldDirectory::open(&data, Access::NotWritableByOthers).unwrap();
        // Between the check and the read, the directory is swapped for another with another file in it.
        std::fs::rename(&data, tmp.path().join("d.old")).unwrap();
        std::fs::create_dir(&data).unwrap();
        std::fs::write(data.join("install.json"), b"replacement").unwrap();
        assert_eq!(
            dir.read("install.json".as_ref(), Access::NotWritableByOthers, 1024).unwrap_err().kind(),
            ErrorKind::PermissionDenied
        );
        assert_eq!(std::fs::read(tmp.path().join("d.old/install.json")).unwrap(), b"genuine");
        assert_eq!(std::fs::read(data.join("install.json")).unwrap(), b"replacement");
        assert!(dir.write("install.json".as_ref(), b"changed").is_err());
        assert!(dir.remove("install.json".as_ref()).is_err());
    }

    #[test]
    fn links_and_loose_files_are_refused() {
        let tmp = tmp();
        let data = tmp.path().join("d");
        std::fs::create_dir(&data).unwrap();
        chmod(&data, 0o700);
        std::fs::write(tmp.path().join("elsewhere.json"), b"x").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("elsewhere.json"), data.join("install.json")).unwrap();
        let dir = HeldDirectory::open(&data, Access::NotWritableByOthers).unwrap();
        assert!(dir.read("install.json".as_ref(), Access::NotWritableByOthers, 1024).is_err(), "a link");
        assert!(dir.read("none.json".as_ref(), Access::NotWritableByOthers, 1024).unwrap().is_none());
        std::fs::write(data.join("loose.json"), b"x").unwrap();
        chmod(&data.join("loose.json"), 0o666);
        assert!(dir.read("loose.json".as_ref(), Access::NotWritableByOthers, 1024).is_err());
        chmod(&data.join("loose.json"), 0o644);
        assert!(dir.read("loose.json".as_ref(), Access::NotWritableByOthers, 1024).is_ok());
        assert!(dir.read("loose.json".as_ref(), Access::Private, 1024).is_err(), "a credential must be private");
        // The directory itself: a link to it, or a loose mode, is refused.
        std::os::unix::fs::symlink(&data, tmp.path().join("link")).unwrap();
        assert!(
            HeldDirectory::open(&tmp.path().join("link"), Access::NotWritableByOthers).is_err(),
            "a linked root is refused"
        );
        chmod(&data, 0o777);
        assert!(HeldDirectory::open(&data, Access::NotWritableByOthers).is_err());
    }

    #[test]
    fn write_is_atomic_and_private() {
        let tmp = tmp();
        chmod(tmp.path(), 0o700);
        let dir = HeldDirectory::open(tmp.path(), Access::Private).unwrap();
        dir.write("local-host.json".as_ref(), b"one").unwrap();
        dir.write("local-host.json".as_ref(), b"two").unwrap();
        assert_eq!(dir.read("local-host.json".as_ref(), Access::Private, 1024).unwrap().unwrap(), b"two");
        assert_eq!(std::fs::metadata(tmp.path().join("local-host.json")).unwrap().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
        dir.remove("local-host.json".as_ref()).unwrap();
        dir.remove("local-host.json".as_ref()).unwrap();
    }

    #[test]
    fn executables_need_a_safe_file_and_ancestry() {
        let tmp = tmp();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        chmod(&bin, 0o755);
        let program = bin.join("node");
        std::fs::write(&program, b"#!/bin/sh\n").unwrap();
        chmod(&program, 0o755);
        assert_eq!(Files::trusted_program(&program).unwrap(), std::fs::canonicalize(&program).unwrap());
        chmod(&program, 0o777);
        assert!(Files::trusted_program(&program).is_err());
        chmod(&program, 0o755);
        chmod(&bin, 0o777);
        assert!(Files::trusted_program(&program).is_err());
        chmod(&bin, 0o755);
        assert_eq!(
            Files::trusted_program(Path::new("/bin/sh")).unwrap(),
            std::fs::canonicalize("/bin/sh").unwrap(),
            "a system link resolved"
        );
    }
}
