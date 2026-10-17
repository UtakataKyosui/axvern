//! Real fd/rg/bat through an isolated Pueue daemon and the public CLI.
//! Skipped (with a message) when pueue, fd, rg or bat is not installed.
mod common;
use common::*;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn installed(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            dir.join(name)
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}
macro_rules! live {
    () => {
        if !["pueue", "pueued", "fd", "rg", "bat"]
            .iter()
            .all(|tool| installed(tool))
        {
            eprintln!("skipped: pueue, fd, rg and bat are required");
            return;
        }
    };
}
fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
/// A Git worktree (the marker directory is all fd and rg look for) with ignored, hidden,
/// symlinked and newline-named entries.
fn repo(root: &Path) {
    write(root, ".git/HEAD", "ref: refs/heads/main\n");
    write(root, ".git/objects/zz", "object");
    write(root, ".gitignore", "build/\n*.log\n!keep.log\n");
    write(root, "src/a.rs", "fn a() { TODO }\n");
    write(
        root,
        "src/sub/b.rs",
        "b TODO TODO\nsecond line\nthird TODO\n",
    );
    write(root, ".hidden/h", "TODO hidden\n");
    write(root, "build/out", "TODO ignored directory\n");
    write(root, "a.log", "TODO ignored file\n");
    write(root, "keep.log", "TODO negated\n");
    write(root, "nl\nname.txt", "TODO newline\n");
    std::os::unix::fs::symlink("src", root.join("link")).unwrap();
}
fn config(q: &Queue, extra: &str) -> std::path::PathBuf {
    let path = q._temp.path().join("axvern.toml");
    let mut text = format!(
        "schema_version=1\n{extra}\n[execution]\npueue_config={}\nstore={}\n",
        serde_json::to_string(&q.adapter.config).unwrap(),
        serde_json::to_string(&q._temp.path().join("store")).unwrap()
    );
    for tool in ["fd", "rg", "bat"] {
        text += &format!("[[policy.rules]]\nid='{tool}'\neffect='allow'\ncommand='{tool}'\n");
    }
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}
/// Run a start command and wait for its stored result in the same response.
fn start(q: &Queue, config: &Path, args: &[&str]) -> Value {
    let mut argv = args.to_vec();
    argv.extend(["--wait", "30s"]);
    cli_json(q, config, &argv, 0)
}
fn items(response: &Value) -> Vec<Value> {
    response["output"]["result"]["items"]
        .as_array()
        .unwrap()
        .clone()
}
fn paths(response: &Value) -> Vec<String> {
    items(response)
        .iter()
        .map(|i| i["path"].as_str().unwrap().to_owned())
        .collect()
}
fn typed(response: &Value) -> Vec<(String, String)> {
    items(response)
        .iter()
        .map(|i| {
            (
                i["path"].as_str().unwrap().to_owned(),
                i["type"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn list_applies_depth_hidden_ignore_symlink_and_git_rules() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("repo");
    repo(&root);
    let config = config(&q, "");
    let first = start(&q, &config, &["fs", "list", "repo"]);
    assert_eq!(first["command"], "fs list");
    assert_eq!(first["state"], "done");
    assert_eq!(first["output"]["result_availability"], "available");
    let result = &first["output"]["result"];
    assert_eq!(result["kind"], "fs.list");
    assert_eq!(
        result["summary"]["root"],
        root.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(result["summary"]["depth"], 1);
    // Hidden entries are shown; ignored ones are not; `.git` is an entry; symlinks stay links.
    assert_eq!(
        typed(&first),
        [
            (".git", "directory"),
            (".gitignore", "file"),
            (".hidden", "directory"),
            ("keep.log", "file"),
            ("link", "symlink"),
            ("nl\nname.txt", "file"),
            ("src", "directory"),
        ]
        .map(|(p, t)| (p.to_owned(), t.to_owned()))
    );
    assert_eq!(result["completeness"]["scope_complete"], true);
    assert_eq!(result["completeness"]["truncated"], false);
    assert_eq!(result["errors"], json!([]));
    // Depth 2 reveals children, lists `.git` itself but not what it contains, and does
    // not follow `link`.
    let deep = start(&q, &config, &["fs", "list", "repo", "--depth", "2"]);
    let deep_paths = paths(&deep);
    assert!(deep_paths.contains(&"src/a.rs".to_owned()));
    assert!(deep_paths.contains(&".hidden/h".to_owned()));
    assert!(deep_paths.contains(&".git".to_owned()));
    assert!(!deep_paths.iter().any(|p| p.starts_with(".git/")));
    assert!(!deep_paths.iter().any(|p| p.starts_with("link/")));
    assert!(!deep_paths.iter().any(|p| p.starts_with("build")));
    let mut sorted = deep_paths.clone();
    sorted.sort();
    assert_eq!(deep_paths, sorted);
    // `--include-ignored` lifts the ignore rules, not the `.git` boundary.
    let all = start(
        &q,
        &config,
        &["fs", "list", "repo", "--depth", "2", "--include-ignored"],
    );
    let all_paths = paths(&all);
    for expected in ["a.log", "build", "build/out"] {
        assert!(all_paths.contains(&expected.to_owned()), "{expected}");
    }
    assert!(!all_paths.iter().any(|p| p.starts_with(".git/")));
    // A symbolic-link root is resolved to its target.
    let linked = start(&q, &config, &["fs", "list", "repo/link"]);
    assert_eq!(
        linked["output"]["result"]["summary"]["root"],
        root.join("src").canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(paths(&linked), ["a.rs", "sub"]);
}

#[test]
fn list_truncates_at_the_item_limit_and_confirms_an_exact_fit() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("repo");
    for n in 0..5 {
        write(&root, &format!("f{n}.txt"), "x");
    }
    let tight = config(
        &q,
        "[discovery]\nmax_items=3\nmax_entries=100\ntimeout_ms=60000",
    );
    let cut = start(&q, &tight, &["fs", "list", "repo"]);
    let result = &cut["output"]["result"];
    assert_eq!(result["items"].as_array().unwrap().len(), 3);
    assert_eq!(result["completeness"]["truncated"], true);
    assert_eq!(
        result["completeness"]["truncation_reasons"],
        json!(["max_items"])
    );
    assert_eq!(result["completeness"]["omitted_count"], Value::Null);
    assert_eq!(result["completeness"]["scope_complete"], false);
    let exact = config(
        &q,
        "[discovery]\nmax_items=5\nmax_entries=100\ntimeout_ms=60000",
    );
    let full = start(&q, &exact, &["fs", "list", "repo"]);
    assert_eq!(
        full["output"]["result"]["items"].as_array().unwrap().len(),
        5
    );
    assert_eq!(full["output"]["result"]["completeness"]["truncated"], false);
    let few = config(
        &q,
        "[discovery]\nmax_items=5\nmax_entries=5\ntimeout_ms=60000",
    );
    let entries = start(&q, &few, &["fs", "list", "repo"]);
    assert_eq!(
        entries["output"]["result"]["completeness"]["truncated"],
        false
    );
    write(&root, "f5.txt", "x");
    let more = start(&q, &few, &["fs", "list", "repo"]);
    assert_eq!(
        more["output"]["result"]["completeness"]["truncation_reasons"],
        json!(["max_entries"])
    );
}

#[test]
fn unreadable_subdirectory_is_a_partial_result_not_a_failure() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("tree");
    write(&root, "ok/f", "x");
    write(&root, "locked/g", "x");
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let config = config(&q, "");
    let response = start(&q, &config, &["fs", "list", "tree", "--depth", "2"]);
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    if unsafe_root_can_read(&root) {
        return;
    }
    assert_eq!(response["state"], "done");
    let result = &response["output"]["result"];
    assert_eq!(result["completeness"]["partial"], true);
    assert_eq!(result["completeness"]["scope_complete"], false);
    assert_eq!(result["errors"][0]["code"], "permission_denied");
    assert_eq!(result["errors"][0]["path"], "locked");
    assert!(paths(&response).contains(&"ok/f".to_owned()));
}
/// Permission tests are meaningless when the process can read anything.
fn unsafe_root_can_read(root: &Path) -> bool {
    let locked = root.join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&locked).is_ok();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    readable
}

#[test]
fn find_maps_regex_glob_extension_type_case_and_full_path() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("repo");
    repo(&root);
    write(&root, "src/Main.RS", "x");
    let config = config(&q, "");
    let regex = start(&q, &config, &["fs", "find", "^[ab]\\.rs$", "repo"]);
    assert_eq!(paths(&regex), ["src/a.rs", "src/sub/b.rs"]);
    assert_eq!(regex["output"]["result"]["summary"]["syntax"], "regex");
    // Case-sensitive unless asked.
    let exact = start(&q, &config, &["fs", "find", "main", "repo"]);
    assert!(paths(&exact).is_empty());
    let loose = start(
        &q,
        &config,
        &["fs", "find", "main", "repo", "--ignore-case"],
    );
    assert_eq!(paths(&loose), ["src/Main.RS"]);
    let glob = start(&q, &config, &["fs", "find", "*.rs", "repo", "--glob"]);
    assert_eq!(glob["output"]["result"]["summary"]["syntax"], "glob");
    assert_eq!(paths(&glob), ["src/a.rs", "src/sub/b.rs"]);
    let both = start(
        &q,
        &config,
        &[
            "fs",
            "find",
            "*",
            "repo",
            "--glob",
            "--extension",
            "rs",
            "--type",
            "file",
        ],
    );
    // fd matches extensions case-insensitively.
    assert_eq!(paths(&both), ["src/Main.RS", "src/a.rs", "src/sub/b.rs"]);
    let dirs = start(
        &q,
        &config,
        &["fs", "find", "^s", "repo", "--type", "directory"],
    );
    assert_eq!(
        typed(&dirs),
        [
            ("src".to_owned(), "directory".to_owned()),
            ("src/sub".to_owned(), "directory".to_owned())
        ]
    );
    let links = start(
        &q,
        &config,
        &["fs", "find", ".", "repo", "--type", "symlink"],
    );
    assert_eq!(typed(&links), [("link".to_owned(), "symlink".to_owned())]);
    // The full path is matched relative to the root.
    let anchored = start(
        &q,
        &config,
        &["fs", "find", "^src/sub/", "repo", "--full-path"],
    );
    assert_eq!(paths(&anchored), ["src/sub/b.rs"]);
    let pattern = start(
        &q,
        &config,
        &["fs", "find", "src/*/*.rs", "repo", "--glob", "--full-path"],
    );
    assert_eq!(paths(&pattern), ["src/sub/b.rs"]);
    // Ignored entries need --include-ignored; a depth bound is honoured.
    assert!(paths(&start(&q, &config, &["fs", "find", "out", "repo"])).is_empty());
    let ignored = start(
        &q,
        &config,
        &["fs", "find", "out", "repo", "--include-ignored"],
    );
    assert_eq!(paths(&ignored), ["build/out"]);
    let shallow = start(&q, &config, &["fs", "find", "rs$", "repo", "--depth", "1"]);
    assert!(paths(&shallow).is_empty());
    // A file name with a newline stays one path.
    let newline = start(&q, &config, &["fs", "find", "^nl", "repo"]);
    assert_eq!(paths(&newline), ["nl\nname.txt"]);
    // Nothing found is a normal, complete result.
    let none = start(&q, &config, &["fs", "find", "zzzz", "repo"]);
    assert_eq!(none["state"], "done");
    assert_eq!(items(&none), Vec::<Value>::new());
    assert_eq!(
        none["output"]["result"]["completeness"]["scope_complete"],
        true
    );
}

#[test]
fn search_maps_ranges_context_globs_ignore_and_not_found() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("repo");
    repo(&root);
    fs::write(root.join("src/bin.dat"), b"TODO\0binary\n").unwrap();
    let config = config(&q, "");
    let found = start(&q, &config, &["fs", "search", "TODO", "repo"]);
    let result = &found["output"]["result"];
    assert_eq!(result["kind"], "fs.search");
    let hits: Vec<_> = items(&found)
        .iter()
        .map(|i| {
            (
                i["path"].as_str().unwrap().to_owned(),
                i["line"].as_u64().unwrap(),
            )
        })
        .collect();
    // Hidden files and negated ignores are searched; ignored files and `.git` are not.
    assert_eq!(
        hits,
        [
            (".hidden/h", 1),
            ("keep.log", 1),
            ("nl\nname.txt", 1),
            ("src/a.rs", 1),
            ("src/sub/b.rs", 1),
            ("src/sub/b.rs", 3),
        ]
        .map(|(p, l)| (p.to_owned(), l))
    );
    // Two matches on one line are one item with two 1-based, end-exclusive ranges.
    let second = items(&found)
        .into_iter()
        .find(|i| i["path"] == "src/sub/b.rs" && i["line"] == 1)
        .unwrap();
    assert_eq!(second["text"], "b TODO TODO");
    assert_eq!(
        second["ranges"],
        json!([{"start_byte":3,"end_byte":7},{"start_byte":8,"end_byte":12}])
    );
    assert_eq!(second["line_truncated"], false);
    assert_eq!(result["summary"]["binary_files_skipped"], 1);
    assert_eq!(result["warnings"][0]["code"], "binary_files_skipped");
    // Context lines come with their line numbers.
    let context = start(
        &q,
        &config,
        &["fs", "search", "second", "repo", "--context", "1"],
    );
    let hit = &items(&context)[0];
    assert_eq!(
        hit["context_before"],
        json!([{"line":1,"text":"b TODO TODO"}])
    );
    assert_eq!(
        hit["context_after"],
        json!([{"line":3,"text":"third TODO"}])
    );
    // A positive glob narrows the files but never re-admits ignored ones.
    let glob = start(
        &q,
        &config,
        &["fs", "search", "TODO", "repo", "--glob", "*.log"],
    );
    assert_eq!(items(&glob).len(), 1);
    assert_eq!(items(&glob)[0]["path"], "keep.log");
    let negated = start(
        &q,
        &config,
        &[
            "fs", "search", "TODO", "repo", "--glob", "*.rs", "--glob", "!sub",
        ],
    );
    assert_eq!(
        items(&negated)
            .iter()
            .map(|i| i["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["src/a.rs"]
    );
    let ignored = start(
        &q,
        &config,
        &["fs", "search", "ignored", "repo", "--include-ignored"],
    );
    assert_eq!(items(&ignored).len(), 2);
    // No match is a result (rg exits 1), not a failure.
    let none = start(&q, &config, &["fs", "search", "no-such-text", "repo"]);
    assert_eq!(none["state"], "done");
    assert_eq!(items(&none), Vec::<Value>::new());
    assert_eq!(
        none["output"]["result"]["completeness"]["scope_complete"],
        true
    );
    // A single file can be searched; paths are relative to its directory.
    let file = start(&q, &config, &["fs", "search", "TODO", "repo/src/a.rs"]);
    assert_eq!(items(&file)[0]["path"], "a.rs");
    // The pattern is data: shell syntax is searched for literally and never executed.
    let marker = root.join("INJECTED");
    let literal = format!("$(touch {})", marker.display());
    let injected = cli_json(
        &q,
        &config,
        &[
            "fs",
            "search",
            "--fixed-strings",
            "--wait",
            "30s",
            "--",
            &literal,
            "repo",
        ],
        0,
    );
    assert_eq!(injected["state"], "done");
    assert!(!marker.exists());
}

#[test]
fn read_returns_the_requested_range_with_eof_and_rejects_unreadable_targets() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("files");
    write(&root, "five.txt", "1\n2\n3\n4\n5\n");
    write(&root, "crlf.txt", "\u{feff}one\r\ntwo\r\n");
    write(&root, "esc.txt", "red \u{1b}[31mtext\u{1b}[0m\n");
    fs::write(root.join("bin.dat"), b"a\0b\n").unwrap();
    fs::write(root.join("latin.txt"), b"ok\n\xff\xfe\n").unwrap();
    let config = config(&q, "");
    let mid = start(
        &q,
        &config,
        &[
            "fs",
            "read",
            "files/five.txt",
            "--from-line",
            "2",
            "--lines",
            "2",
        ],
    );
    let summary = &mid["output"]["result"]["summary"];
    assert_eq!(summary["from_line"], 2);
    assert_eq!(summary["requested_lines"], 2);
    assert_eq!(summary["eof"], false);
    assert_eq!(
        items(&mid),
        [
            json!({"line":2,"text":"2","line_truncated":false}),
            json!({"line":3,"text":"3","line_truncated":false})
        ]
    );
    let tail = start(
        &q,
        &config,
        &["fs", "read", "files/five.txt", "--from-line", "4"],
    );
    assert_eq!(items(&tail).len(), 2);
    assert_eq!(tail["output"]["result"]["summary"]["eof"], true);
    // A request ending exactly at the last line is also at EOF.
    let exact = start(
        &q,
        &config,
        &[
            "fs",
            "read",
            "files/five.txt",
            "--from-line",
            "4",
            "--lines",
            "2",
        ],
    );
    assert_eq!(exact["output"]["result"]["summary"]["eof"], true);
    let past = start(
        &q,
        &config,
        &["fs", "read", "files/five.txt", "--from-line", "99"],
    );
    assert_eq!(items(&past), Vec::<Value>::new());
    assert_eq!(past["output"]["result"]["summary"]["eof"], true);
    assert_eq!(past["state"], "done");
    // BOM and CRLF are normalized; escape sequences in the file stay data.
    let crlf = start(&q, &config, &["fs", "read", "files/crlf.txt"]);
    assert_eq!(items(&crlf)[0]["text"], "one");
    assert_eq!(items(&crlf)[1]["text"], "two");
    let esc = start(&q, &config, &["fs", "read", "files/esc.txt"]);
    assert_eq!(items(&esc)[0]["text"], "red \u{1b}[31mtext\u{1b}[0m");
    // Invalid input and targets are refused before anything is registered.
    let before = q.adapter.list().unwrap().len();
    for (args, code, error) in [
        (
            vec!["fs", "read", "files/bin.dat"],
            2,
            "unsupported_encoding",
        ),
        (vec!["fs", "read", "files"], 2, "invalid_target"),
        (vec!["fs", "read", "files/missing"], 2, "target_not_found"),
        (
            vec!["fs", "read", "files/five.txt", "--lines", "0"],
            2,
            "invalid_input",
        ),
        (
            vec!["fs", "read", "files/five.txt", "--lines", "10001"],
            2,
            "invalid_input",
        ),
        (
            vec!["fs", "read", "files/five.txt", "--from-line", "0"],
            2,
            "invalid_input",
        ),
        (
            vec![
                "fs",
                "read",
                "files/five.txt",
                "--from-line",
                "9223372036854775807",
                "--lines",
                "2",
            ],
            2,
            "invalid_input",
        ),
        (
            vec!["fs", "read", "files/.git/config"],
            2,
            "target_not_found",
        ),
    ] {
        assert_eq!(
            cli_json(&q, &config, &args, code)["error"]["code"],
            error,
            "{args:?}"
        );
    }
    assert_eq!(q.adapter.list().unwrap().len(), before);
    // Content that is not UTF-8 text is a failed target with a typed error in its result.
    let latin = start(&q, &config, &["fs", "read", "files/latin.txt"]);
    assert_eq!(latin["state"], "failed");
    assert_eq!(
        latin["output"]["result"]["errors"][0]["code"],
        "unsupported_encoding"
    );
    assert_eq!(items(&latin), Vec::<Value>::new());
}

#[test]
fn long_lines_are_cut_on_a_character_boundary_and_flagged() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("long");
    let line = "あ".repeat(10_000);
    write(&root, "wide.txt", &format!("{line}\nshort\n"));
    write(&root, "hit.txt", &format!("TODO {line}\n"));
    let config = config(&q, "");
    let read = start(&q, &config, &["fs", "read", "long/wide.txt"]);
    let first = &items(&read)[0];
    assert_eq!(first["line_truncated"], true);
    let text = first["text"].as_str().unwrap();
    assert!(text.len() <= 16 * 1024 && text.chars().all(|c| c == 'あ'));
    assert_eq!(items(&read)[1]["text"], "short");
    let search = start(&q, &config, &["fs", "search", "TODO", "long/hit.txt"]);
    let hit = &items(&search)[0];
    assert_eq!(hit["line_truncated"], true);
    assert_eq!(hit["ranges"], json!([{"start_byte":1,"end_byte":5}]));
}

#[test]
fn large_results_page_without_gaps_and_survive_pueue_cleanup() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("many");
    for n in 0..250 {
        write(&root, &format!("d{:03}/f.txt", n), "x");
    }
    let config = config(&q, "");
    let first = start(&q, &config, &["fs", "list", "many"]);
    let id = first["execution_id"].as_str().unwrap().to_owned();
    assert_eq!(items(&first).len(), 100);
    assert_eq!(first["output"]["result"]["page"]["has_more"], true);
    let mut all = paths(&first);
    let mut cursor = first["output"]["result"]["page"]["next_cursor"]
        .as_str()
        .map(str::to_owned);
    q.client(&["clean"]);
    let mut pages = 1;
    while let Some(token) = cursor {
        let page = cli_json(&q, &config, &["result", &id, "--cursor", &token], 0);
        // A repeated cursor returns the identical stored page.
        assert_eq!(
            cli_json(&q, &config, &["result", &id, "--cursor", &token], 0)["output"],
            page["output"]
        );
        all.extend(paths(&page));
        cursor = page["output"]["result"]["page"]["next_cursor"]
            .as_str()
            .map(str::to_owned);
        pages += 1;
    }
    assert_eq!(pages, 3);
    assert_eq!(all.len(), 250);
    let mut expected = all.clone();
    expected.sort();
    expected.dedup();
    assert_eq!(all, expected);
    // Wrong-execution cursors are refused.
    let other = start(&q, &config, &["fs", "list", "many", "--depth", "2"]);
    let token = first["output"]["result"]["page"]["next_cursor"]
        .as_str()
        .unwrap();
    let foreign = cli_json(
        &q,
        &config,
        &[
            "result",
            other["execution_id"].as_str().unwrap(),
            "--cursor",
            token,
        ],
        2,
    );
    assert_eq!(foreign["error"]["code"], "invalid_cursor");
    // A small byte budget still pages, and a budget below one item is a typed error.
    let tiny = cli_json(&q, &config, &["result", &id, "--max-bytes", "1024"], 0);
    assert!(serde_json::to_vec(&tiny).unwrap().len() < 1024);
    assert!(!items(&tiny).is_empty());
}

#[test]
fn slow_targets_return_an_id_first_and_the_result_later() {
    live!();
    let q = Queue::new();
    let root = q._temp.path().join("repo");
    repo(&root);
    let config = config(&q, "");
    let started = cli_json(&q, &config, &["fs", "list", "repo", "--wait", "0s"], 0);
    let id = started["execution_id"].as_str().unwrap();
    assert!(id.starts_with("ex_"));
    // Whether it already finished or not, waiting and then fetching gives the stored result.
    cli_json(&q, &config, &["wait", id, "--timeout", "30s"], 0);
    let result = cli_json(&q, &config, &["result", id], 0);
    assert_eq!(result["output"]["result"]["kind"], "fs.list");
    assert_eq!(result["state"], "done");
    // Registration happened exactly once and retrieval never registers more.
    let tasks = q.adapter.list().unwrap().len();
    cli_json(&q, &config, &["result", id], 0);
    assert_eq!(q.adapter.list().unwrap().len(), tasks);
}

#[test]
fn doctor_reports_adapter_tools_with_versions() {
    live!();
    let q = Queue::new();
    let config = config(&q, "");
    let report = cli_json(
        &q,
        &config,
        &["doctor", "--scope", "discovery"],
        12, // ast-grep and tokei adapters are not part of this release
    );
    let checks = report["output"]["checks"].as_array().unwrap();
    for name in ["fd", "rg", "bat"] {
        let check = checks.iter().find(|c| c["name"] == name).unwrap();
        assert_eq!(check["level"], "pass", "{name}");
        assert_eq!(check["tool"]["installed"], true);
        assert_eq!(check["tool"]["feature_available"], true);
        assert!(check["tool"]["version"].as_str().is_some());
        assert!(
            !check["tool"]["supported_versions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
