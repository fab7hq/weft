//! A running app holds its version's folder, so a daemon that starts on a
//! newer version never removes the app from under it.

use weft_desktop::version;

#[test]
fn the_apps_version_is_kept_while_it_runs_and_removable_after() {
    let tmp = tempfile::tempdir().unwrap();
    let versions = tmp.path().join("versions");
    let app = versions.join("0.1.0/Weft.app/Contents/MacOS");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("weft-desktop"), "").unwrap();
    std::fs::create_dir_all(versions.join("0.2.0")).unwrap();

    let held = version::hold(&versions, &app.join("weft-desktop")).expect("held");
    let removed = weftd::versions::prune(&versions, &[versions.join("0.2.0")]);
    assert!(removed.is_empty(), "{removed:?}");
    assert!(app.join("weft-desktop").is_file());
    drop(held);
    let removed = weftd::versions::prune(&versions, &[versions.join("0.2.0")]);
    assert_eq!(removed.len(), 1, "once it has quit");
}

#[test]
fn a_build_outside_the_versions_holds_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let versions = tmp.path().join("versions");
    std::fs::create_dir_all(&versions).unwrap();
    assert!(version::hold(&versions, &tmp.path().join("target/weft-desktop")).is_none());
}
