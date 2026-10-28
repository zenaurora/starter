use super::*;
use super::{
    process::Environment,
    providers::{parse_brew, parse_node},
};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

const BREW: &str = r#"{"formulae":[
 {"name":"deno","full_name":"deno","installed":[{"version":"2.9.7"}],"linked_keg":"2.9.7","versions":{"stable":"2.9.8"},"revision":1,"pinned":true,"outdated":true},
 {"name":"head","full_name":"head","installed":[{"version":"HEAD-abc"}],"versions":{"stable":"1.0"},"pinned":false,"outdated":false}
],"casks":[
 {"token":"app","full_token":"app","homepage":null,"installed":"1","version":"2","outdated":true},
 {"token":"self","full_token":"self","installed":"1","version":"latest","outdated":false,"auto_updates":true}
]}"#;
fn script(root: &Path, name: &str, source: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\n{source}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}
fn snapshot(items: Vec<Software>, environment: Environment) -> Snapshot {
    Snapshot {
        items: items.into_iter().map(Arc::new).collect(),
        environment,
        ..Snapshot::default()
    }
}
#[test]
fn brew_preserves_provider_flags_and_does_not_call_skipped_casks_current() {
    let batch = parse_brew(BREW, Path::new("/opt/homebrew/bin/brew")).unwrap();
    assert_eq!(batch.items.len(), 4);
    assert_eq!(batch.items[0].latest.as_deref(), Some("2.9.8_1"));
    assert!(!batch.items[0].can_update());
    assert_eq!(batch.items[1].state, UpdateState::Managed);
    assert!(batch.items[2].can_update());
    assert_eq!(batch.items[2].homepage, "");
    assert_eq!(batch.items[3].state, UpdateState::Manual);
    assert!(!batch.items[3].can_update());
    assert!(parse_brew("{}", Path::new("/bin/brew")).is_err());
}
#[test]
fn self_updating_casks_compare_live_versions_and_do_not_offer_downgrades() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Example.app");
    fs::create_dir_all(path.join("Contents")).unwrap();
    let inventory=serde_json::json!({"formulae":[],"casks":[{
        "token":"example","full_token":"example","installed":"1.0","version":"2.0","outdated":false,"auto_updates":true,
        "artifacts":[{"app":["Example.app",{"target":path}]}]
    }]}).to_string();
    let info = |version: &str| {
        plist::Value::Dictionary(plist::Dictionary::from_iter([(
            "CFBundleShortVersionString",
            plist::Value::String(version.into()),
        )]))
        .to_file_xml(path.join("Contents/Info.plist"))
        .unwrap()
    };
    info("1.5");
    let batch = parse_brew(&inventory, Path::new("/opt/homebrew/bin/brew")).unwrap();
    assert_eq!(batch.items[0].installed, "1.5");
    assert_eq!(batch.items[0].state, UpdateState::Available);
    assert!(batch.items[0].can_update());
    let Action::Command(command) = &batch.items[0].action else {
        panic!("expected manager action")
    };
    assert_eq!(
        command.args,
        ["upgrade", "--cask", "--greedy", "--", "example"]
    );
    info("3.0");
    let batch = parse_brew(&inventory, Path::new("/opt/homebrew/bin/brew")).unwrap();
    assert_eq!(batch.items[0].state, UpdateState::Managed);
    assert!(!batch.items[0].can_update());
}
#[test]
fn node_instances_keep_roots_and_linked_packages_are_manual() {
    let json = r#"[{"path":"/env/a","dependencies":{"same":{"version":"1.0"},"linked":{"version":"link:../project","link":true}}},{"path":"/env/b","dependencies":{"same":{"version":"2.0"}}}]"#;
    let batch = parse_node(json, Path::new("/bin/pnpm"), Source::Pnpm).unwrap();
    let instances: Vec<_> = batch.items.iter().filter(|i| i.name == "same").collect();
    assert_ne!(instances[0].id, instances[1].id);
    assert!(instances.iter().all(|i| i.state == UpdateState::Unchecked));
    assert!(
        !batch
            .items
            .iter()
            .find(|i| i.name == "linked")
            .unwrap()
            .can_update()
    );
    assert!(
        parse_node(
            r#"{"error":{"code":"ENOENT"}}"#,
            Path::new("/bin/npm"),
            Source::Npm
        )
        .is_err()
    );
}
#[test]
fn bun_global_inventory_accepts_empty_and_populated_lists_but_rejects_unknown_output() {
    let root = tempfile::tempdir().unwrap();
    script(root.path(), "bun", r#"/bin/cat "$HOME/bun-list""#);
    for (output, names, warning) in [
        (
            "/fixture/.bun/install/global node_modules (0 installed)\n",
            vec![],
            false,
        ),
        (
            "/fixture/.bun/install/global node_modules (0)\n",
            vec![],
            false,
        ),
        (
            "/fixture/.bun/install/global node_modules (2 installed)\n├── typescript@5.0.4\n└── @example/tool@1.2.3\n",
            vec!["typescript", "@example/tool"],
            false,
        ),
        ("Unexpected output (0 installed)\n", vec![], true),
    ] {
        fs::write(root.path().join("bun-list"), output).unwrap();
        let batch = providers::scan(
            &Environment::fixture(root.path(), root.path()),
            ScanMode::Local,
            &AtomicBool::new(false),
        );
        assert_eq!(!batch.warnings.is_empty(), warning, "{:?}", batch.warnings);
        assert_eq!(
            batch
                .items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            names,
        );
    }
}
#[test]
fn plans_require_explicit_valid_selection_and_exclude_pins_manual_and_unknown_ids() {
    let root = tempfile::tempdir().unwrap();
    let env = Environment::fixture(root.path(), root.path());
    let inventory = snapshot(
        parse_brew(BREW, Path::new("/opt/homebrew/bin/brew"))
            .unwrap()
            .items,
        env,
    );
    let id = inventory.items[2].id.clone();
    let plan = Plan::selected(&inventory, &HashSet::from([id.clone()])).unwrap();
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].id, id);
    assert!(Plan::selected(&inventory, &HashSet::new()).is_err());
    for id in [
        &inventory.items[0].id,
        &inventory.items[1].id,
        &inventory.items[3].id,
        &"removed".into(),
    ] {
        assert!(Plan::selected(&inventory, &HashSet::from([id.clone()])).is_err());
    }
    assert!(Plan::pin(&inventory, &inventory.items[0]).is_some());
}
#[test]
fn requests_pass_literal_argv_and_runtime_environment_without_a_shell() {
    let root = tempfile::tempdir().unwrap();
    let env = Environment::fixture(root.path(), root.path());
    let mut items = parse_brew(BREW, Path::new("/bin/printf")).unwrap().items;
    let literal = "a'$(touch SHOULD_NOT_EXIST)\n--arg";
    items[2].action = Action::Command(Invocation::new(
        Path::new("/usr/bin/printf"),
        ["%s", literal],
    ));
    let inventory = snapshot(items, env);
    let plan = Plan::selected(&inventory, &HashSet::from([inventory.items[2].id.clone()])).unwrap();
    let request = plan.request(0);
    let output = Command::new(&request.program)
        .args(&request.args)
        .env_clear()
        .envs(request.env)
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), literal);
    assert!(!root.path().join("SHOULD_NOT_EXIST").exists());
}
#[test]
fn cache_reuses_snapshot_and_runs_no_more_processes_until_invalidated() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("inventory.json"), BREW).unwrap();
    script(
        root.path(),
        "brew",
        "printf '%s\\n' \"$*\" >> \"$HOME/calls\"\n/bin/cat \"$HOME/inventory.json\"",
    );
    let service = Inventory::default();
    let cancel = AtomicBool::new(false);
    let first = service.scan_using(
        ScanMode::Cached,
        &[],
        &cancel,
        Environment::fixture(root.path(), root.path()),
    );
    let second = service.scan_using(
        ScanMode::Cached,
        &[],
        &cancel,
        Environment::fixture(root.path(), root.path()),
    );
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(
        fs::read_to_string(root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(!first.items[0].search.is_empty());
    service.invalidate();
    let third = service.scan_using(
        ScanMode::Cached,
        &[],
        &cancel,
        Environment::fixture(root.path(), root.path()),
    );
    assert!(!Arc::ptr_eq(&first, &third));
    assert_eq!(
        fs::read_to_string(root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert!(!third.checked_online);
    let mut changed_env = Environment::fixture(root.path(), root.path());
    changed_env
        .variables
        .push(("NPM_CONFIG_PREFIX".into(), root.path().join("other").into()));
    let fourth = service.scan_using(ScanMode::Cached, &[], &cancel, changed_env);
    assert!(!Arc::ptr_eq(&third, &fourth));
    assert_eq!(
        fs::read_to_string(root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        3
    );
}
#[test]
fn revalidation_refuses_changed_versions_and_newly_pinned_packages() {
    let root = tempfile::tempdir().unwrap();
    let json = BREW.replace("\"pinned\":true", "\"pinned\":false");
    fs::write(root.path().join("inventory.json"), &json).unwrap();
    let program = script(root.path(), "brew", "/bin/cat \"$HOME/inventory.json\"");
    let env = Environment::fixture(root.path(), root.path());
    let inventory = snapshot(parse_brew(&json, &program).unwrap().items, env);
    let plan = Plan::selected(&inventory, &HashSet::from([inventory.items[0].id.clone()])).unwrap();
    plan.validate(&AtomicBool::new(false)).unwrap();
    fs::write(root.path().join("inventory.json"), BREW).unwrap();
    assert!(plan.validate(&AtomicBool::new(false)).is_err());
    fs::write(
        root.path().join("inventory.json"),
        json.replace("2.9.7", "2.9.9"),
    )
    .unwrap();
    assert!(plan.validate(&AtomicBool::new(false)).is_err());
}
#[test]
fn npm_outdated_exit_one_is_a_candidate_but_missing_records_stay_unknown() {
    let root = tempfile::tempdir().unwrap();
    script(
        root.path(),
        "npm",
        r#"case "$1" in
 list) printf '%s' '{"path":"/env/a","dependencies":{"a":{"version":"1"},"b":{"version":"1"}}}' ;;
 outdated) printf '%s' '{"a":{"current":"1","wanted":"2","latest":"3"}}'; exit 1 ;;
 *) exit 99 ;;
esac"#,
    );
    let batch = providers::scan(
        &Environment::fixture(root.path(), root.path()),
        ScanMode::Check,
        &AtomicBool::new(false),
    );
    let a = batch.items.iter().find(|i| i.name == "a").unwrap();
    let b = batch.items.iter().find(|i| i.name == "b").unwrap();
    assert_eq!(a.state, UpdateState::Available);
    assert_eq!(a.latest.as_deref(), Some("2"));
    assert_eq!(b.state, UpdateState::Unchecked);
    assert!(b.latest.is_none());
}
#[test]
fn failed_sources_are_isolated_and_legacy_pipx_is_never_maintained() {
    let root = tempfile::tempdir().unwrap();
    script(root.path(), "brew", "printf broken");
    script(
        root.path(),
        "npm",
        r#"printf '%s' '{"dependencies":{"tool":{"version":"1"}}}'"#,
    );
    script(
        root.path(),
        "pipx",
        r#"printf '%s\n' "$*" >> "$HOME/pipx-calls"; printf 'usage: pipx list --json'"#,
    );
    let batch = providers::scan(
        &Environment::fixture(root.path(), root.path()),
        ScanMode::Local,
        &AtomicBool::new(false),
    );
    assert!(batch.items.iter().any(|i| i.name == "tool"));
    assert_eq!(batch.warnings.len(), 2);
    assert_eq!(
        fs::read_to_string(root.path().join("pipx-calls"))
            .unwrap()
            .trim(),
        "list --help"
    );
}
#[test]
fn timeout_output_limit_and_cancel_terminate_detection_without_hanging() {
    let root = tempfile::tempdir().unwrap();
    let env = Environment::fixture(root.path(), root.path());
    let cancel = AtomicBool::new(false);
    let start = Instant::now();
    let error = env
        .capture(
            &Invocation::new(Path::new("/bin/sh"), ["-c", "sleep 30"]),
            Duration::from_millis(80),
            &cancel,
            &[0],
        )
        .unwrap_err();
    assert!(error.to_string().contains("超时"));
    assert!(start.elapsed() < Duration::from_secs(2));
    let error = env
        .capture(
            &Invocation::new(
                Path::new("/bin/sh"),
                ["-c", "dd if=/dev/zero bs=1048576 count=17 2>/dev/null"],
            ),
            Duration::from_secs(3),
            &cancel,
            &[0],
        )
        .unwrap_err();
    assert!(error.to_string().contains("超过限制"));
    cancel.store(true, Ordering::Relaxed);
    assert!(
        env.run(Path::new("/nonexistent"), &[], &cancel)
            .unwrap_err()
            .to_string()
            .contains("取消")
    );
}
#[test]
fn failures_and_interruptions_stop_the_queue_and_preserve_each_outcome() {
    let mut outcomes = vec![Outcome::Pending; 4];
    assert!(finish_step(&mut outcomes, 0, 0, false));
    assert!(!finish_step(&mut outcomes, 1, 2, false));
    assert_eq!(
        outcomes,
        vec![
            Outcome::Succeeded,
            Outcome::Failed(2),
            Outcome::Skipped,
            Outcome::Skipped
        ]
    );
    let mut outcomes = vec![Outcome::Pending; 2];
    assert!(!finish_step(&mut outcomes, 0, 0, true));
    assert_eq!(outcomes, vec![Outcome::Stopped, Outcome::Skipped]);
}
#[test]
fn application_inventory_reuses_catalog_and_never_guesses_a_download() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Test.app");
    fs::create_dir_all(path.join("Contents")).unwrap();
    let info = plist::Value::Dictionary(
        [
            (
                "CFBundleIdentifier".to_string(),
                plist::Value::String("example.test".into()),
            ),
            (
                "CFBundleShortVersionString".to_string(),
                plist::Value::String("1.2".into()),
            ),
            (
                "SUFeedURL".to_string(),
                plist::Value::String("https://example.test/feed.xml".into()),
            ),
        ]
        .into_iter()
        .collect(),
    );
    info.to_file_xml(path.join("Contents/Info.plist")).unwrap();
    let app = Application {
        id: "test".into(),
        name: "Test".into(),
        path: path.clone(),
        aliases: vec![],
    };
    let mut batch = providers::Batch::default();
    apps::append(std::slice::from_ref(&app), &mut batch);
    let item = batch.items.iter().find(|i| i.name == "Test").unwrap();
    assert_eq!(item.installed, "1.2");
    assert_eq!(item.state, UpdateState::Unchecked);
    assert!(item.latest.is_none());
    assert!(matches!(item.action, Action::OpenApp(_)));
    assert!(item.homepage.is_empty());
    let mut batch = providers::Batch::default();
    batch.owned.push(path.canonicalize().unwrap());
    apps::append(&[app], &mut batch);
    assert!(!batch.items.iter().any(|i| i.name == "Test"));
}

#[test]
fn node_custom_sources_and_unrelated_outdated_records_do_not_become_registry_updates() {
    let root = tempfile::tempdir().unwrap();
    script(
        root.path(),
        "npm",
        r#"case "$1" in
list) printf '%s' '{"path":"/env/a","dependencies":{
 "registry":{"name":"registry","version":"1","resolved":"https://registry.npmjs.org/registry/-/registry-1.tgz"},
 "git":{"version":"1","resolved":"https://github.com/org/git/archive/v1.tar.gz"},
 "alias":{"name":"different","version":"1"},
 "linked":{"version":"1","resolved":"file:../linked"},
 "stale":{"version":"1"}, "other-location":{"version":"1"}
}}' ;;
outdated) printf '%s' '{"registry":[{"current":"1","wanted":"2","location":"/env/b/node_modules/registry"},{"current":"1","wanted":"3","location":"/env/a/node_modules/registry"}],"stale":{"current":"0","wanted":"9"},"other-location":{"current":"1","wanted":"9","location":"/env/b/node_modules/other-location"}}'; exit 1 ;;
*) exit 99 ;;
esac"#,
    );
    let batch = providers::scan(
        &Environment::fixture(root.path(), root.path()),
        ScanMode::Check,
        &AtomicBool::new(false),
    );
    let find = |name| batch.items.iter().find(|i| i.name == name).unwrap();
    assert_eq!(find("registry").latest.as_deref(), Some("3"));
    for name in ["git", "alias", "linked"] {
        assert!(!find(name).can_update());
    }
    for name in ["stale", "other-location"] {
        assert_eq!(find(name).state, UpdateState::Unchecked);
    }
}

#[test]
fn tool_managers_preserve_environment_names_paths_and_pinned_toolchains() {
    let root = tempfile::tempdir().unwrap();
    let tools = root.path().join("custom-tools");
    script(
        root.path(),
        "uv",
        &format!(
            "printf '%s\\n' 'example v1.2 ({}/example)' '- example'",
            tools.display()
        ),
    );
    script(
        root.path(),
        "pipx",
        r#"case "$2" in
--help) printf '%s' '--skip-maintenance' ;;
--skip-maintenance) printf '%s' '{"venvs":{"cli-suffix":{"metadata":{"main_package":{"package":"cli","package_version":"1.2","app_paths":[{"__Path__":"/env/pipx/bin/cli-suffix"}]}}}}}' ;;
*) exit 99 ;;
esac"#,
    );
    script(
        root.path(),
        "rustup",
        r#"case "$1" in
toolchain) printf '%s\n' 'stable-aarch64-apple-darwin (default)' 'nightly-2026-10-01-aarch64-apple-darwin' '1.90.0-aarch64-apple-darwin' ;;
check) printf '%s\n' 'stable-aarch64-apple-darwin - Update available : 1.90 -> 1.91'; exit 100 ;;
*) exit 99 ;;
esac"#,
    );
    let mut env = Environment::fixture(root.path(), root.path());
    env.variables
        .push(("UV_TOOL_DIR".into(), tools.as_os_str().into()));
    let batch = providers::scan(&env, ScanMode::Check, &AtomicBool::new(false));
    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    let uv = batch.items.iter().find(|i| i.source == Source::Uv).unwrap();
    assert_eq!(uv.path, Some(tools.join("example")));
    assert!(batch.owns(&tools.join("example/bin/example")));
    let pipx = batch
        .items
        .iter()
        .find(|i| i.source == Source::Pipx)
        .unwrap();
    let Action::Command(command) = &pipx.action else {
        panic!("expected per-environment command")
    };
    assert_eq!(command.args, ["upgrade", "cli-suffix"]);
    assert!(batch.owns(Path::new("/env/pipx/bin/cli-suffix")));
    let rust: Vec<_> = batch
        .items
        .iter()
        .filter(|i| i.source == Source::Rustup)
        .collect();
    assert_eq!(rust[0].state, UpdateState::Available);
    assert!(rust[0].can_update());
    assert!(rust[1..].iter().all(|i| i.pinned && !i.can_update()));
}

#[test]
fn cargo_reconstructs_recorded_options_and_keeps_incomplete_or_custom_installs_manual() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().join("cargo-custom");
    fs::create_dir_all(&install).unwrap();
    let metadata = serde_json::json!({"version_req":null,"bins":["one"],"features":["fast"],"all_features":false,"no_default_features":true,"profile":"release","target":"aarch64-apple-darwin"});
    let mut pinned = metadata.clone();
    pinned["version_req"] = "=1.0".into();
    let mut incomplete = metadata.clone();
    incomplete["target"] = Value::Null;
    fs::write(
        install.join(".crates2.json"),
        serde_json::to_vec(&serde_json::json!({"installs":{
            "normal 1.0 (registry+https://github.com/rust-lang/crates.io-index)":metadata,
            "pinned 1.0 (registry+https://github.com/rust-lang/crates.io-index)":pinned,
            "incomplete 1.0 (registry+https://github.com/rust-lang/crates.io-index)":incomplete,
            "git 1.0 (git+https://example.test/repo)":metadata
        }}))
        .unwrap(),
    )
    .unwrap();
    script(root.path(), "cargo", "exit 99");
    let mut env = Environment::fixture(root.path(), root.path());
    env.variables
        .push(("CARGO_INSTALL_ROOT".into(), install.as_os_str().into()));
    let batch = providers::scan(&env, ScanMode::Local, &AtomicBool::new(false));
    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    let find = |name| batch.items.iter().find(|i| i.name == name).unwrap();
    let Action::Command(command) = &find("normal").action else {
        panic!("expected recorded options")
    };
    for pair in [
        ["--version", "*"],
        ["--features", "fast"],
        ["--bin", "one"],
        ["--target", "aarch64-apple-darwin"],
        ["--profile", "release"],
    ] {
        assert!(command.args.windows(2).any(|args| args == pair));
    }
    assert!(command.args.contains(&"--no-default-features".into()));
    assert!(find("pinned").pinned);
    for name in ["pinned", "incomplete", "git"] {
        assert!(!find(name).can_update());
    }
    assert!(batch.owns(&install.join("bin/one")));
}

#[test]
fn uv_self_update_requires_receipt_for_exact_installation_and_does_not_duplicate_an_owner() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let program = script(&bin, "uv", "exit 99");
    let config = root.path().join(".config/uv");
    fs::create_dir_all(&config).unwrap();
    let receipt = |prefix: &Path| {
        serde_json::to_vec(
            &serde_json::json!({"install_prefix":prefix,"version":"1.0","binaries":["uv","uvx"]}),
        )
        .unwrap()
    };
    fs::write(config.join("uv-receipt.json"), receipt(root.path())).unwrap();
    let env = Environment::fixture(root.path(), &bin);
    let mut batch = providers::Batch::default();
    providers::append_self_updaters(&env, &mut batch);
    let uv = batch.items.iter().find(|i| i.name == "uv").unwrap();
    assert!(uv.can_update());
    assert_eq!(uv.installed, "1.0");
    let mut validation_env = env.clone();
    validation_env
        .variables
        .push(("CARGO_INSTALL_ROOT".into(), root.path().as_os_str().into()));
    let inventory = snapshot(batch.items.clone(), validation_env);
    let plan = Plan::selected(&inventory, &HashSet::from([uv.id.clone()])).unwrap();
    plan.validate(&AtomicBool::new(false)).unwrap();
    script(&bin, "cargo", "exit 99");
    fs::write(
        root.path().join(".crates2.json"),
        r#"{"installs":{"uv 1.0 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["uv"]}}}"#,
    )
    .unwrap();
    assert!(
        plan.validate(&AtomicBool::new(false)).is_err(),
        "a newly registered manager must take ownership before a standalone update"
    );
    let mut owned = providers::Batch::default();
    owned.owned.push(program.canonicalize().unwrap());
    providers::append_self_updaters(&env, &mut owned);
    assert!(!owned.items.iter().any(|i| i.name == "uv"));
    fs::write(
        config.join("uv-receipt.json"),
        receipt(root.path().parent().unwrap()),
    )
    .unwrap();
    let mut mismatch = providers::Batch::default();
    providers::append_self_updaters(&env, &mut mismatch);
    assert!(
        !mismatch
            .items
            .iter()
            .find(|i| i.name == "uv")
            .unwrap()
            .can_update()
    );
}
