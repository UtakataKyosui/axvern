//! Optional display-only eza spike; mandatory structured tools have their own live suites.
mod common;
use axvern_core::adapters::tools::{self, ToolStatus};
use common::*;
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

#[test]
fn eza_replacement_output_is_saved_as_display_text_without_parsing_filenames() {
    if tools::locate("eza".as_ref()).is_none() {
        assert!(
            std::env::var_os("AXVERN_REQUIRE_EZA_SPIKE").is_none(),
            "eza 0.23.5 required for the display spike"
        );
        eprintln!("skipped: optional eza display spike (install eza 0.23.5)");
        return;
    }
    assert!(matches!(
        tools::detect(&tools::EZA, Duration::from_secs(5)),
        ToolStatus::Supported(_)
    ));
    let q = Queue::new();
    let root = q._temp.path().join("names");
    fs::create_dir(&root).unwrap();
    for name in ["space name", "line\nname", "back\\slash", "日本語"] {
        fs::write(root.join(name), "").unwrap();
    }
    let config = q._temp.path().join("axvern.toml");
    fs::write(&config, format!(
        "schema_version=1\n[replacement]\nenabled=true\nrules=['ls-to-eza']\n[execution]\npueue_config={}\nstore={}\n[[policy.rules]]\nid='source'\neffect='allow'\ncommand='ls'\n[[policy.rules]]\nid='target'\neffect='allow'\ncommand='eza'\n",
        serde_json::to_string(&q.adapter.config).unwrap(),
        serde_json::to_string(&q._temp.path().join("store")).unwrap()
    )).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let done = cli_json(
        &q,
        &config,
        &["run", "--wait", "30s", "--", "ls", "names"],
        0,
    );
    assert_eq!(done["state"], "done");
    let id = done["execution_id"].as_str().unwrap();
    let saved = cli_json(&q, &config, &["result", id], 0);
    let items = saved["output"]["result"]["items"].as_array().unwrap();
    let text: String = items
        .iter()
        .map(|item| {
            assert!(
                item.get("path").is_none(),
                "display text must not be decoded into paths"
            );
            item["text"].as_str().unwrap()
        })
        .collect();
    assert!(text.contains("space name") && text.contains("日本語"));
    assert!(
        !text.contains('\u{1b}'),
        "noninteractive display must not emit ANSI colors"
    );
    assert_eq!(
        cli_json(&q, &config, &["result", id], 0)["output"],
        saved["output"]
    );
    assert_eq!(
        q.adapter.list().unwrap().len(),
        1,
        "reading saved display text never reruns eza"
    );
}
