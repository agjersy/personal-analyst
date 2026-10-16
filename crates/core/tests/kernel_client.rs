//! Integration tests for `KernelClient` against the stdlib-only fake kernel
//! (`tests/fake_kernel.py`), launched through an injected command line the way
//! `ANALYST_TOOLING_PYTHON` would select the interpreter.

use analyst_core::kernel::{KernelArgs, KernelClient, LocalStdio};
use analyst_protocol::kernel::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn fake_kernel() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_kernel.py")
}

fn launcher() -> Vec<String> {
    // Mirrors `ANALYST_TOOLING_PYTHON`-style injection: the first element is the interpreter.
    let py = std::env::var("ANALYST_TOOLING_PYTHON").unwrap_or_else(|_| "python3".to_string());
    vec![py, fake_kernel().to_string_lossy().to_string()]
}

struct Fixture {
    _home: tempfile::TempDir,
    _repo: tempfile::TempDir,
    transport: LocalStdio,
    root: PathBuf,
}

fn fixture(script: &str) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join("scripts")).unwrap();
    std::fs::write(repo.path().join("scripts/hello.py"), script).unwrap();
    let runs = home.path().join("runs");
    let args = KernelArgs {
        python: "python3".into(),
        root: repo.path().to_string_lossy().to_string(),
        runs_dir: runs.to_string_lossy().to_string(),
        out_dir: None,
        sandbox: Some("none".into()),
    };
    Fixture {
        root: repo.path().to_path_buf(),
        transport: LocalStdio::with_launcher(launcher(), args),
        _home: home,
        _repo: repo,
    }
}

const FAST_SCRIPT: &str = r#"
import os, sys
print("hello from the script")
print("second line", flush=True)
out = os.environ["ANALYST_OUT_DIR"]
os.makedirs(os.path.join(out, "figures"), exist_ok=True)
with open(os.path.join(out, "figures", "fig-1.png"), "wb") as f:
    f.write(b"\x89PNG not really")
print("args:", sys.argv[1:])
"#;

const SLOW_SCRIPT: &str = r#"
import time
for i in range(6):
    print("tick", i, flush=True)
    time.sleep(0.4)
"#;

#[tokio::test]
async fn hello_info_index_run_and_shutdown() {
    let fx = fixture(FAST_SCRIPT);
    let client = KernelClient::connect(Box::new(fx.transport.clone()), "test")
        .await
        .expect("connect");
    let hello = client.hello().await.unwrap();
    assert_eq!(hello.protocol_version, 1);
    assert_eq!(hello.root, fx.root.to_string_lossy());
    let info = client.env_info().await.unwrap();
    assert_eq!(info.python, "python3");
    let idx = client.scripts_index(None).await.unwrap();
    assert_eq!(idx.scripts[0].name, "fit_decay");
    assert_eq!(idx.scripts[0].options[1].dest, "seed");
    let lib = client.lib_index(None).await.unwrap();
    assert!(lib.functions[0].has_math);

    // Unknown method -> RPC error surfaces as KernelError::Rpc.
    let err = client
        .call_value("nope.method", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("-32601"));

    let mut rx = client.subscribe();
    let started = client
        .run_start(RunStartParams {
            script: "scripts/hello.py".into(),
            args: Some(vec!["--x".into(), "1".into()]),
            timeout_s: Some(30.0),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(started.out_dir.starts_with("out/hello/"));

    let (done, stdout, _stderr, artifacts) =
        analyst_core::evals::wait_for_run(&mut rx, &started.run_id, Duration::from_secs(20)).await;
    let done = done.expect("run.done");
    assert_eq!(done.status, RunStatus::Done);
    assert_eq!(done.exit_code, Some(0));
    assert!(stdout.contains("hello from the script"));
    assert!(stdout.contains("args: ['--x', '1']"));
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, ArtifactKind::Figure);
    assert!(artifacts[0].rendered);
    assert!(fx.root.join(&artifacts[0].path).is_file());
    assert!(client.active_runs().await.is_empty());

    let meta = client.run_status(&started.run_id).await.unwrap();
    assert_eq!(meta.status, RunStatus::Done);
    let list = client.runs_list(Some(5)).await.unwrap();
    assert_eq!(list.runs[0].run_id, started.run_id);
    let not_found = client.run_status("missing").await.unwrap_err();
    assert!(not_found.to_string().contains("1000"));

    let ex = client.exec_snippet("print(6*7)", None).await.unwrap();
    assert!(ex.ok);
    assert_eq!(ex.stdout.trim(), "42");

    client.shutdown().await.unwrap();
    assert!(!client.is_connected().await);
}

#[tokio::test]
async fn reconnect_reattaches_running_runs() {
    let fx = fixture(SLOW_SCRIPT);
    let client = KernelClient::connect(Box::new(fx.transport.clone()), "test")
        .await
        .expect("connect");
    let mut rx = client.subscribe();
    let started = client
        .run_start(RunStartParams {
            script: "scripts/hello.py".into(),
            timeout_s: Some(30.0),
            ..Default::default()
        })
        .await
        .unwrap();
    // Wait for the first stdout chunk, then drop the kernel and respawn.
    let mut before = String::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Ok(KernelNotification::Stdout(o))) if o.run_id == started.run_id => {
                before.push_str(&o.chunk);
                break;
            }
            Ok(Ok(_)) => continue,
            _ => panic!("no stdout before reconnect"),
        }
    }
    let offsets_before = client.active_runs().await;
    assert_eq!(offsets_before.len(), 1);
    assert!(offsets_before[0].1.stdout > 0);

    let hello = client.reconnect("test").await.expect("reconnect");
    assert_eq!(hello.protocol_version, 1);
    assert!(client.is_connected().await);

    // After reconnect, run.attach replays from the recorded offsets: the remaining ticks and
    // run.done arrive on the same subscription.
    let (done, after, _, _) =
        analyst_core::evals::wait_for_run(&mut rx, &started.run_id, Duration::from_secs(20)).await;
    let done = done.expect("run.done after reconnect");
    assert_eq!(done.status, RunStatus::Done);
    let all = format!("{before}{after}");
    for i in 0..6 {
        assert!(
            all.contains(&format!("tick {i}")),
            "missing tick {i} in {all:?}"
        );
    }
    assert_eq!(all.matches("tick 0").count(), 1, "no duplicate replay");
    assert!(client.active_runs().await.is_empty());
    client.shutdown().await.unwrap();
}
