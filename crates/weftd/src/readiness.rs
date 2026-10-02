//! Asking a harness what it has installed.
//!
//! Reading the answer is a rule and lives in [`weft_core::readiness`].
//! Running the command that produces one is here, because that is the only
//! part that needs a process.

use serde_json::Value;
use weft_core::harness::Harness;
pub use weft_core::readiness::*;

use crate::outside::Outside;

/// Ask one harness. `cli` is the global check, made once by the caller.
pub fn check(out: &dyn Outside, h: &Harness, cli: bool) -> Readiness {
    look(out, h, cli).0
}

/// Its readiness, and the `rf` version it has installed, from one listing.
pub fn look(out: &dyn Outside, h: &Harness, cli: bool) -> (Readiness, Option<String>) {
    if !cli {
        return (Readiness::Missing(Gap::Cli), None);
    }
    let Some(listing) = ask(out, h) else { return (Readiness::Unknown, None) };
    let state = read(h, &listing);
    if let Some(version) = weft_core::sync::installed(h, &listing) {
        return (state, Some(version));
    }
    // A listing that gives no version leaves it to the installed plugin's own
    // `plugin.json`, where the harness file says it is.
    let Some(at) = h.manifest.as_deref() else { return (state, None) };
    use crate::harness::OnThisMachine;
    match out.newest(&h.config_home().join(at)) {
        // No plugin there, whatever the listing proved (Cursor's lists only
        // the marketplace): the plugin itself is still to be installed.
        None if state == Readiness::Ready => (Readiness::Missing(Gap::Plugin), None),
        None => (state, None),
        // A plugin from before it carried a version is older than any.
        Some(text) => (
            state,
            Some(weft_core::sync::manifest_version(&text).unwrap_or_else(|| UNVERSIONED.into())),
        ),
    }
}

/// What an installed plugin whose `plugin.json` gives no version reads as:
/// behind every release, so its update is offered.
pub const UNVERSIONED: &str = "unversioned";

/// What the harness said, or nothing at all when it would not say.
fn ask(outside: &dyn Outside, h: &Harness) -> Option<Value> {
    let argv = h.argv(&h.list);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = outside.run(&h.program, &args, None).ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok().or_else(|| {
        // Some harnesses say "nothing installed" in words: that is an empty
        // listing, not a harness that would not answer.
        let said = String::from_utf8_lossy(&out.stdout);
        let none = h.listing.as_ref()?.none.as_deref()?;
        (said.trim() == none).then(|| serde_json::json!({}))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outside::Fixed;
    use serde_json::json;
    use weft_core::readiness::Gap;

    fn absent() -> crate::harness::Harness {
        let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        crate::harness::Harness {
            name: "nowhere".into(),
            title: "Nowhere".into(),
            program: "definitely-not-a-real-binary-weft".into(),
            args: Vec::new(),
            config_env: Some("NOWHERE_HOME".into()),
            config_default: ".nowhere".into(),
            list: argv(&["plugin", "list", "--json"]),
            add_marketplace: Some(argv(&["x"])),
            install_plugin: Some(argv(&["y"])),
            update_marketplace: Some(argv(&["z"])),
            update_plugin: Some(argv(&["w"])),
            listing: Some(weft_core::harness::Listing {
                installed: argv(&["installed"]),
                available: argv(&["available"]),
                name: "id".into(),
                on: argv(&["enabled"]),
                none: None,
                want: Vec::new(),
            }),
            resume: argv(&["resume"]),
            transcript: None,
            turbo: Vec::new(),
            manifest: None,
            beta: false,
        }
    }

    #[test]
    fn no_cli_is_the_first_gap_and_nothing_else_is_asked() {
        assert_eq!(check(&Fixed::new(), &absent(), false), Readiness::Missing(Gap::Cli));
    }

    #[test]
    fn a_harness_that_will_not_answer_is_unknown_rather_than_missing() {
        // `read` never sees the output, because there was none. The distinction
        // matters: Missing can be fixed by installing, Unknown cannot.
        assert_eq!(
            read(&absent(), &json!("not a listing at all")),
            Readiness::Missing(Gap::Marketplace)
        );
        assert_eq!(check(&Fixed::new(), &absent(), true), Readiness::Unknown);
    }

    /// A listing that gives no version (Antigravity's imports, Cursor's
    /// marketplaces) leaves it to the plugin's own `plugin.json`, where the
    /// harness file says it is: the newest match of a `*` folder wins.
    #[test]
    fn a_listing_without_a_version_reads_it_from_the_installed_manifest() {
        let mut h = absent();
        h.program = "nowhere".into();
        let listing = r#"{"installed": [{"id": "rf@fab7", "enabled": true}], "available": []}"#;
        let world = |files: &[(&str, &str)]| {
            files
                .iter()
                .fold(Fixed::new().answers("nowhere", &["list"], 0, listing), |w, (p, t)| {
                    w.file(p, t)
                })
        };
        assert_eq!(look(&world(&[]), &h, true), (Readiness::Ready, None), "no manifest named");

        h.manifest = Some("plugins/cache/fab7/rf/*/.cursor-plugin/plugin.json".into());
        let old =
            ("plugins/cache/fab7/rf/aaa/.cursor-plugin/plugin.json", r#"{"version": "0.1.2"}"#);
        let new =
            ("plugins/cache/fab7/rf/bbb/.cursor-plugin/plugin.json", r#"{"version": "0.1.3"}"#);
        assert_eq!(look(&world(&[old, new]), &h, true).1.as_deref(), Some("0.1.3"));
        let unversioned =
            ("plugins/cache/fab7/rf/ccc/.cursor-plugin/plugin.json", r#"{"name": "rf"}"#);
        assert_eq!(
            look(&world(&[unversioned]), &h, true),
            (Readiness::Ready, Some(UNVERSIONED.into())),
            "a plugin from before it carried a version"
        );
        assert!(weft_core::sync::older(UNVERSIONED, "0.1.3"), "so it is behind any release");
        assert_eq!(
            look(&world(&[]), &h, true),
            (Readiness::Missing(Gap::Plugin), None),
            "the listing proves the marketplace, and no plugin is there"
        );

        let listed = r#"{"installed": [{"id": "rf@fab7", "enabled": true, "version": "0.1.1"}]}"#;
        let both = Fixed::new().answers("nowhere", &["list"], 0, listed).file(new.0, new.1);
        assert_eq!(
            look(&both, &h, true).1.as_deref(),
            Some("0.1.1"),
            "the listing's own comes first"
        );
    }
}
