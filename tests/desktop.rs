use confuse::{
    model::{Record, Trace},
    store::Store,
};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::tempdir;

struct Desktop {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    id: u64,
}

impl Desktop {
    fn launch(data: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_confuse"))
            .args(["desktop", data.to_str().unwrap()])
            .env("CONFUSE_PORT", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut app = Self {
            child,
            input,
            output,
            id: 0,
        };
        assert_eq!(app.receive()["ready"], true);
        app
    }
    fn receive(&mut self) -> Value {
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    fn request(&mut self, action: &str, payload: Value) -> Value {
        self.id += 1;
        writeln!(
            self.input,
            "{}",
            json!({"id":self.id,"channel":action,"payload":payload})
        )
        .unwrap();
        self.input.flush().unwrap();
        let result = self.receive();
        assert_eq!(result["id"], self.id);
        assert!(result.get("error").is_none(), "{result}");
        result["result"].clone()
    }
    fn wait_for(&mut self, action: &str, condition: impl Fn(&Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(12);
        loop {
            let data = self.request(action, json!({}));
            if condition(&data) {
                return data;
            }
            assert!(
                Instant::now() < until,
                "timed out waiting for {action}: {data}"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }
    fn quit(&mut self) {
        assert_eq!(self.request("desktop.quit", json!({})), true);
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                return;
            }
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(30));
        }
    }
}
impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn append(path: &Path, entries: &[Value]) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for entry in entries {
        writeln!(file, "{entry}").unwrap();
    }
}

#[test]
fn desktop_backend_quits_and_recovers_agent_delta_from_disk() {
    let root = tempdir().unwrap();
    let data = root.path().join("state");
    let agents = root.path().join("sessions");
    fs::create_dir(&agents).unwrap();
    let log = agents.join("session.jsonl");
    append(
        &log,
        &[
            json!({"type":"session","version":3,"id":"fixture"}),
            json!({"type":"message","id":"old","message":{"role":"user","content":"existing message"}}),
        ],
    );
    let mut app = Desktop::launch(&data);
    let state = app.request("desktop.get-state", json!({}));
    assert_eq!(state["onboarded"], false);
    let url = state["sdkEndpoint"].as_str().unwrap();
    let address: SocketAddr = url.trim_start_matches("http://").parse().unwrap();
    assert!(TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok());
    app.request(
        "desktop.complete-onboarding",
        json!({"selections":[{"agent":"pi","root":agents,"history":false}]}),
    );
    app.wait_for("notifications.list", |value| {
        value.as_array().is_some_and(|rows| rows.len() == 1)
    });
    assert_eq!(app.request("traces.list", json!({}))["total"], 0);
    app.quit();
    assert!(TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_err());

    append(
        &log,
        &[
            json!({"type":"message","id":"live","parentId":"old","message":{"role":"user","content":"appended while closed"}}),
            json!({"type":"message","id":"reply","parentId":"live","message":{"role":"assistant","content":"fixture response"}}),
        ],
    );
    let mut resumed = Desktop::launch(&data);
    assert_eq!(
        resumed.request("desktop.get-state", json!({}))["onboarded"],
        true
    );
    let result = resumed.wait_for("traces.list", |value| value["total"] == 1);
    assert_eq!(result["data"][0]["input"], "appended while closed");
    assert_eq!(result["data"][0]["output"], "fixture response");
    resumed.quit();
}

#[test]
fn upstream_calls_preserve_sdk_keys_and_scope_data_after_upgrade() {
    let root = tempdir().unwrap();
    let data = root.path().join("state");
    let sqlite = data.join("confuse.sqlite");
    let store = Store::open(&sqlite).unwrap();
    let saved = store.create_key("local", "sdk").unwrap();
    let other = store.create_project("Second").unwrap();
    for (project, trace_id) in [("local", "local-trace"), (other.id.as_str(), "other-trace")] {
        store
            .ingest(
                project,
                &[Record {
                    trace: Trace {
                        id: trace_id.to_owned(),
                        name: "upgrade fixture".to_owned(),
                        environment: "default".to_owned(),
                        start_ns: 1_798_883_200_000_000_000,
                        ..Trace::default()
                    },
                    root: true,
                    observation: None,
                }],
            )
            .unwrap();
    }
    drop(store);
    rusqlite::Connection::open(&sqlite)
        .unwrap()
        .execute("DROP TABLE key_attributes", [])
        .unwrap();

    let mut app = Desktop::launch(&data);
    let user = app.request("langfuse.session", json!({}));
    let projects = user["projects"].as_array().unwrap();
    assert_eq!(
        projects
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["local", &other.id]
    );
    let keys = app.request(
        "langfuse.request",
        json!({
            "procedure":"projectApiKeys.byProjectId","input":{"projectId":"local"}
        }),
    );
    assert_eq!(keys["data"][0]["publicKey"], saved.public_key);
    assert!(keys["data"][0].get("secretKey").is_none());
    let missing = app.request(
        "langfuse.request",
        json!({
            "procedure":"traces.byId","input":{"projectId":"local","traceId":"other-trace"}
        }),
    );
    assert_eq!(missing["error"]["code"], "NOT_FOUND");
    let filtered = app.request("langfuse.request", json!({
        "procedure":"traces.all","input":{"projectId":other.id,"page":0,"limit":1,
            "filter":[{"column":"name","type":"string","value":"upgrade fixture","operator":"="}]}
    }));
    assert_eq!(filtered["data"]["traces"][0]["id"], "other-trace");
    assert!(
        filtered["dates"]
            .as_array()
            .unwrap()
            .contains(&json!("traces.0.timestamp"))
    );
    let unset = app.request(
        "langfuse.request",
        json!({
            "procedure":"traces.countAll","input":{}
        }),
    );
    assert_eq!(unset["error"]["code"], "BAD_REQUEST");
    app.quit();
    let store = Store::open(&sqlite).unwrap();
    assert_eq!(
        store
            .project_for_key(&saved.public_key, &saved.secret_key)
            .unwrap()
            .as_deref(),
        Some("local")
    );
}
