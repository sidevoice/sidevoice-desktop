// The former local trusted implementation's regressions exercise the shared owner on every host.
use sidevoice_desktop_test_support::platform::storage::{Others, Replacement, StorageFixture, StorageFixtures};
use sidevoice_platform::fs::{Access, Create, Directory, Files, Fs, HeldDirectory};
use std::{fs, io::ErrorKind};

fn tmp() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    StorageFixtures::private(tmp.path());
    tmp
}

#[test]
fn ancestry_others_can_write_is_refused_unless_protected() {
    let tmp = tmp();
    let shared = tmp.path().join("shared");
    let data = shared.join("sidevoice");
    Files::ensure_private_dir(&data).unwrap();
    StorageFixtures::readable(&shared);
    assert!(HeldDirectory::open(&data, Access::NotWritableByOthers).is_ok());
    StorageFixtures::writable(&shared);
    let error = HeldDirectory::open(&data, Access::NotWritableByOthers).err().expect("a writable parent passed");
    assert_eq!(error.kind(), ErrorKind::PermissionDenied);
    StorageFixtures::group_writable(&shared);
    assert!(HeldDirectory::open(&data, Access::NotWritableByOthers).is_err(), "a group write grant");
    if StorageFixtures::protect_shared_ancestor(&shared) {
        assert!(HeldDirectory::open(&data, Access::NotWritableByOthers).is_ok(), "protected shared ancestor");
    }
    assert_eq!(
        HeldDirectory::open(&tmp.path().join("none"), Access::Private).err().unwrap().kind(),
        ErrorKind::NotFound
    );
}

#[test]
fn a_directory_replaced_after_it_was_opened_is_not_read() {
    let tmp = tmp();
    let data = tmp.path().join("d");
    Files::create_private_dir(&data).unwrap();
    StorageFixtures::record(&data.join("install.json"), b"genuine");
    let dir = HeldDirectory::open(&data, Access::NotWritableByOthers).unwrap();
    match StorageFixtures::replace_directory(&data, &tmp.path().join("d.old")) {
        Replacement::Replaced => {
            Files::create_private_dir(&data).unwrap();
            StorageFixtures::record(&data.join("install.json"), b"replacement");
            assert_eq!(
                dir.read("install.json".as_ref(), Access::NotWritableByOthers, 1024).unwrap_err().kind(),
                ErrorKind::PermissionDenied
            );
            assert_eq!(fs::read(tmp.path().join("d.old/install.json")).unwrap(), b"genuine");
            assert_eq!(fs::read(data.join("install.json")).unwrap(), b"replacement");
            assert!(dir.write("install.json".as_ref(), b"changed").is_err());
            assert!(dir.remove("install.json".as_ref()).is_err());
        }
        Replacement::Prevented => {
            assert!(!tmp.path().join("d.old").exists());
            assert_eq!(
                dir.read("install.json".as_ref(), Access::NotWritableByOthers, 1024).unwrap().unwrap(),
                b"genuine"
            );
            assert_eq!(fs::read(data.join("install.json")).unwrap(), b"genuine");
        }
    }
}

#[test]
fn links_and_loose_files_are_refused() {
    let tmp = tmp();
    let data = tmp.path().join("d");
    Files::create_private_dir(&data).unwrap();
    StorageFixtures::record(&tmp.path().join("elsewhere.json"), b"x");
    StorageFixtures::symlink(tmp.path().join("elsewhere.json"), data.join("install.json"));
    let dir = HeldDirectory::open(&data, Access::NotWritableByOthers).unwrap();
    assert!(dir.read("install.json".as_ref(), Access::NotWritableByOthers, 1024).is_err(), "a link");
    assert!(dir.read("none.json".as_ref(), Access::NotWritableByOthers, 1024).unwrap().is_none());
    StorageFixtures::record(&data.join("loose.json"), b"x");
    StorageFixtures::writable(&data.join("loose.json"));
    assert!(dir.read("loose.json".as_ref(), Access::NotWritableByOthers, 1024).is_err());
    StorageFixtures::private(&data.join("loose.json"));
    StorageFixtures::readable(&data.join("loose.json"));
    assert!(dir.read("loose.json".as_ref(), Access::NotWritableByOthers, 1024).is_ok());
    assert!(dir.read("loose.json".as_ref(), Access::Private, 1024).is_err(), "a credential must be private");
    StorageFixtures::symlink(&data, tmp.path().join("link"));
    assert!(
        HeldDirectory::open(&tmp.path().join("link"), Access::NotWritableByOthers).is_err(),
        "a linked root is refused"
    );
    StorageFixtures::writable(&data);
    assert!(HeldDirectory::open(&data, Access::NotWritableByOthers).is_err());
}

#[test]
fn write_is_atomic_and_private() {
    let tmp = tmp();
    let dir = HeldDirectory::open(tmp.path(), Access::Private).unwrap();
    dir.write("local-host.json".as_ref(), b"one").unwrap();
    dir.write("local-host.json".as_ref(), b"two").unwrap();
    assert_eq!(dir.read("local-host.json".as_ref(), Access::Private, 1024).unwrap().unwrap(), b"two");
    StorageFixtures::assert_private(&tmp.path().join("local-host.json"));
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    dir.remove("local-host.json".as_ref()).unwrap();
    dir.remove("local-host.json".as_ref()).unwrap();
}

#[test]
fn executables_need_a_safe_file_and_ancestry() {
    let tmp = tmp();
    let bin = tmp.path().join("bin");
    Files::create_private_dir(&bin).unwrap();
    StorageFixtures::readable(&bin);
    let program = bin.join("fixture.exe");
    Files::open_private(&program, Create::NewProgram).unwrap();
    StorageFixtures::readable(&program);
    assert_eq!(Files::trusted_program(&program).unwrap(), fs::canonicalize(&program).unwrap());
    StorageFixtures::writable(&program);
    assert!(Files::trusted_program(&program).is_err());
    assert_eq!(StorageFixtures::others(&program), Others::Change, "never repaired");
    StorageFixtures::private(&program);
    StorageFixtures::readable(&program);
    StorageFixtures::writable(&bin);
    assert!(Files::trusted_program(&program).is_err());
    StorageFixtures::private(&bin);
    StorageFixtures::readable(&bin);
    let system = StorageFixtures::system_program();
    assert_eq!(
        Files::trusted_program(&system).unwrap(),
        fs::canonicalize(&system).unwrap(),
        "system installs remain accepted"
    );
}

#[test]
fn hard_linked_records_are_never_read_repaired_or_removed() {
    let tmp = tmp();
    let dir = HeldDirectory::open(tmp.path(), Access::Private).unwrap();
    dir.write("record".as_ref(), b"genuine").unwrap();
    fs::hard_link(tmp.path().join("record"), tmp.path().join("alias")).unwrap();
    assert!(dir.read("record".as_ref(), Access::Private, 1024).is_err());
    assert!(dir.write("record".as_ref(), b"changed").is_err());
    assert!(dir.remove("record".as_ref()).is_err());
    assert_eq!(fs::read(tmp.path().join("record")).unwrap(), b"genuine");
    assert_eq!(fs::read(tmp.path().join("alias")).unwrap(), b"genuine");
}

#[test]
fn records_work_below_search_only_ancestry_without_repair() {
    let tmp = tmp();
    let data = tmp.path().join("data");
    Files::create_private_dir(&data).unwrap();
    StorageFixtures::with_search_only_ancestor(tmp.path(), || {
        let dir = HeldDirectory::open(&data, Access::Private).unwrap();
        dir.write("record".as_ref(), b"genuine").unwrap();
        assert_eq!(dir.read("record".as_ref(), Access::Private, 1024).unwrap().unwrap(), b"genuine");
        dir.remove("record".as_ref()).unwrap();
    });
}
