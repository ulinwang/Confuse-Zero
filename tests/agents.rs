use confuse::{
    agents::{Agent, Collector, Discovery, Selection},
    model::{TraceFilter, usage_total},
    store::{DEFAULT_PROJECT, Store},
};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};
use tempfile::tempdir;

const TIME: &str = "2026-09-01T12:00:00Z";

fn lines(path: &Path, data: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for value in data {
        writeln!(file, "{value}").unwrap();
    }
}

fn pi(id: &str, parent: Option<&str>, role: &str, content: &str, usage: Value) -> Value {
    json!({"type":"message","id":id,"parentId":parent,"timestamp":TIME,"message":{"role":role,"content":content,"model":"fixture","usage":usage}})
}

#[test]
fn pi_discovery_uses_package_identity_without_launching_executable() {
    let temp = tempdir().unwrap();
    let home = temp.path();
    let bin = home.join("bin");
    let manager = home.join("fnm");
    let bundle = manager.join("lib/node_modules/@earendil-works/pi-coding-agent");
    let executable = bundle.join("dist/bundle/cli.js");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(
        bundle.join("package.json"),
        r#"{"name":"@earendil-works/pi-coding-agent","version":"1.6.0"}"#,
    )
    .unwrap();
    fs::write(&executable, "not run").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let collision = bin.join("pi");
    fs::write(&collision, "same name").unwrap();
    fs::set_permissions(&collision, fs::Permissions::from_mode(0o755)).unwrap();
    let resolved = manager.join("pi");
    symlink(executable, &resolved).unwrap();
    let configured = home.join("pi-archive");
    fs::create_dir_all(home.join(".pi/agent")).unwrap();
    fs::create_dir(&configured).unwrap();
    fs::write(
        home.join(".pi/agent/settings.json"),
        json!({"sessionDir":configured}).to_string(),
    )
    .unwrap();
    let result = Discovery {
        home: home.to_owned(),
        bin_dirs: vec![bin, manager],
        applications: vec![],
    }
    .find(Agent::Pi);
    assert_eq!(result.executable.as_deref(), Some(resolved.as_path()));
    assert_eq!(result.version.as_deref(), Some("1.6.0"));
    assert_eq!(result.root, configured);
    assert!(result.root_readable);
}

#[test]
fn pi_history_is_optional_and_relaunch_reads_only_committed_delta() {
    let temp = tempdir().unwrap();
    let db = temp.path().join("test.sqlite");
    let root = temp.path().join("agent");
    let file = root.join("work/session.jsonl");
    lines(
        &file,
        &[
            json!({"type":"session","id":"session-a","version":3}),
            pi("old", None, "user", "historical", Value::Null),
        ],
    );
    let store = Store::open(&db).unwrap();
    let collector = Collector::new(store.clone());
    collector
        .select(
            DEFAULT_PROJECT,
            &[Selection {
                agent: Agent::Pi,
                root: root.clone(),
                history: false,
            }],
        )
        .unwrap();
    collector.update(DEFAULT_PROJECT).unwrap();
    assert_eq!(
        store
            .traces(DEFAULT_PROJECT, &TraceFilter::default(), 20, 0)
            .unwrap()
            .1,
        0
    );
    assert_eq!(store.notifications(DEFAULT_PROJECT).unwrap().len(), 1);
    store.mark_notifications_read(DEFAULT_PROJECT).unwrap();
    drop(collector);
    drop(store);

    lines(
        &file,
        &[
            pi("a", Some("old"), "user", "new input", Value::Null),
            pi(
                "b",
                Some("a"),
                "assistant",
                "new output",
                json!({"input":10,"output":2}),
            ),
            pi(
                "c",
                Some("a"),
                "assistant",
                "alternate output",
                json!({"input":10,"output":3}),
            ),
        ],
    );
    let partial = pi("d", Some("b"), "toolResult", "tool output", Value::Null).to_string();
    let split = partial.len() / 2;
    let mut writer = OpenOptions::new().append(true).open(&file).unwrap();
    writer.write_all(&partial.as_bytes()[..split]).unwrap();
    let store = Store::open(&db).unwrap();
    let collector = Collector::new(store.clone());
    collector.update(DEFAULT_PROJECT).unwrap();
    assert_eq!(
        store
            .observations(DEFAULT_PROJECT, None, None, 20, 0)
            .unwrap()
            .1,
        3
    );
    writer.write_all(&partial.as_bytes()[split..]).unwrap();
    writer.write_all(b"\n").unwrap();
    collector.update(DEFAULT_PROJECT).unwrap();
    collector.update(DEFAULT_PROJECT).unwrap();
    let (events, count) = store
        .observations(DEFAULT_PROJECT, None, None, 20, 0)
        .unwrap();
    assert_eq!(count, 4);
    assert_eq!(
        events
            .iter()
            .filter_map(|row| usage_total(&row.usage))
            .sum::<i64>(),
        25
    );
    let user = events
        .iter()
        .find(|row| row.input == "new input" && row.kind == "EVENT")
        .unwrap();
    assert!(
        events
            .iter()
            .filter(|row| row.kind == "GENERATION")
            .all(|row| row.parent_id.as_deref() == Some(&user.id))
    );
    assert_eq!(
        store
            .traces(DEFAULT_PROJECT, &TraceFilter::default(), 20, 0)
            .unwrap()
            .1,
        1
    );
    assert_eq!(
        store.notifications(DEFAULT_PROJECT).unwrap()[0]["read"],
        true
    );
}

#[test]
fn current_agent_records_use_the_shared_trace_model() {
    let temp = tempdir().unwrap();
    let cases = [
        (
            Agent::KimiCode,
            "sessions/project/session/agents/main/wire.jsonl",
            false,
            vec![
                json!({"timestamp":TIME,"message":{"type":"TurnBegin","payload":{"user_input":"hello"}}}),
                json!({"timestamp":TIME,"message":{"type":"ContentPart","payload":{"type":"text","text":"world"}}}),
                json!({"timestamp":TIME,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":5,"output":4}}}}),
                json!({"timestamp":TIME,"message":{"type":"StepEnd","payload":{}}}),
            ],
        ),
        (
            Agent::Codex,
            "archived_sessions/rollout-2026-session.jsonl.zst",
            true,
            vec![
                json!({"type":"session_meta","timestamp":TIME,"payload":{"id":"codex-fixture"}}),
                json!({"type":"turn_context","timestamp":TIME,"payload":{"turn_id":"turn","model":"fixture"}}),
                json!({"type":"response_item","timestamp":TIME,"payload":{"role":"user","type":"message","content":[{"type":"input_text","text":"hello"}]}}),
                json!({"type":"response_item","timestamp":TIME,"payload":{"role":"assistant","type":"message","content":[{"type":"output_text","text":"world"}]}}),
                json!({"type":"event_msg","timestamp":TIME,"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5,"output_tokens":4}}}}),
            ],
        ),
        (
            Agent::ClaudeCode,
            "projects/work/claude.jsonl",
            false,
            vec![
                json!({"type":"user","sessionId":"claude-fixture","uuid":"prompt","timestamp":TIME,"message":{"role":"user","content":"hello"}}),
                json!({"type":"assistant","sessionId":"claude-fixture","uuid":"reply","parentUuid":"prompt","timestamp":TIME,
                "message":{"role":"assistant","id":"model-call","model":"fixture","content":[{"type":"text","text":"world"}],
                "usage":{"input_tokens":5,"output_tokens":4,"cache_read_input_tokens":3}}}),
            ],
        ),
        (
            Agent::DeepSeekHarness,
            "sessions/work/session-dsh/session.v4.jsonl.zstd",
            true,
            vec![
                json!({"type":"session","id":"dsh-fixture","version":4,"createdAt":TIME}),
                json!({"type":"turn/start","seq":0,"time":TIME,"data":{"turn":1}}),
                json!({"type":"user/message","seq":1,"time":TIME,"data":{"content":[{"type":"text","text":"hello"}]}}),
                json!({"type":"assistant/message","seq":2,"time":TIME,"data":{"message":{"source":{"model":"fixture"},"content":[{"type":"text","text":"world"}]},
                "turn":1,"usage":{"inputTokens":5,"outputTokens":4}}}),
            ],
        ),
    ];
    let store = Store::memory().unwrap();
    let collector = Collector::new(store.clone());
    for (agent, relative, compressed, data) in cases {
        let root = temp.path().join(agent.id());
        let file = root.join(relative);
        if compressed {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            let mut blob = String::new();
            for row in data {
                blob.push_str(&row.to_string());
                blob.push('\n');
            }
            fs::write(&file, zstd::stream::encode_all(blob.as_bytes(), 1).unwrap()).unwrap();
        } else {
            lines(&file, &data);
        }
        collector
            .select(
                DEFAULT_PROJECT,
                &[Selection {
                    agent,
                    root,
                    history: true,
                }],
            )
            .unwrap();
    }
    collector.update(DEFAULT_PROJECT).unwrap();
    collector.update(DEFAULT_PROJECT).unwrap();
    for kind in [
        Agent::KimiCode,
        Agent::Codex,
        Agent::ClaudeCode,
        Agent::DeepSeekHarness,
    ] {
        let (traces, total) = store
            .traces(
                DEFAULT_PROJECT,
                &TraceFilter {
                    source: Some(kind.label()),
                    ..Default::default()
                },
                10,
                0,
            )
            .unwrap();
        assert_eq!(total, 1, "{kind:?}");
        let (events, count) = store
            .observations(DEFAULT_PROJECT, Some(&traces[0].id), None, 20, 0)
            .unwrap();
        assert!(count >= 2);
        assert!(
            events.iter().any(|row| row.kind == "GENERATION"
                && !row.input.is_null()
                && !row.output.is_null())
        );
        assert_eq!(
            events
                .iter()
                .filter_map(|row| usage_total(&row.usage))
                .sum::<i64>(),
            9,
            "{kind:?}"
        );
    }
    assert_eq!(store.notifications(DEFAULT_PROJECT).unwrap().len(), 4);
}
