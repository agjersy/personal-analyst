//! Service-level integration test against the REAL kernel (`python/kernel`), on a temp repo
//! built from the template: open a session, index, write a compliant script, check it, run it
//! and collect the run events and a captured figure, list figures, extract equations.
//!
//! Skips (with a message) when no tooling interpreter is available:
//! `ANALYST_TOOLING_PYTHON`, else `<workspace>/python/kernel/.venv/bin/python`.

use analyst_core::service::{Workbench, WorkbenchOptions};
use analyst_protocol::kernel::{ArtifactKind, RunStatus, Severity};
use analyst_protocol::ui::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

fn tooling_python() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ANALYST_TOOLING_PYTHON") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    // Do not canonicalize: the venv's `python` is a symlink to the base interpreter, and the
    // venv is only active when started through the symlink.
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())?;
    let p = workspace.join("python/kernel/.venv/bin/python");
    p.is_file().then_some(p)
}

const SCRIPT: &str = r#""""Simulate exponential decay and plot it."""

import argparse

import matplotlib.pyplot as plt
import numpy as np

from analysis.models import single_exp


def main():
    parser = argparse.ArgumentParser(description="Simulate exponential decay")
    parser.add_argument("--tau", type=float, default=2.0, help="time constant")
    parser.add_argument("--t-max", type=float, default=10.0, help="end of the time axis")
    parser.add_argument("--n", type=int, default=50, help="number of samples")
    parser.add_argument("--seed", type=int, default=0, help="random seed")
    args = parser.parse_args()
    rng = np.random.default_rng(args.seed)
    t = np.linspace(0, args.t_max, args.n)
    y = single_exp(t, 1.0, args.tau) + 0.01 * rng.standard_normal(args.n)
    print("samples", args.n, flush=True)
    fig, ax = plt.subplots()
    ax.plot(t, y)
    ax.set_title("decay")
    ax.set_xlabel("t")
    plt.show()
    print("done", flush=True)


if __name__ == "__main__":
    main()
"#;

const MODELS: &str = r#""""Model functions."""

import numpy as np


def single_exp(t, a, tau):
    """Single exponential decay.

    Math:
        y = a * exp(-t / tau)
    """
    return a * np.exp(-t / tau)
"#;

async fn wait_for<F>(rx: &mut broadcast::Receiver<SessionEvent>, secs: u64, pred: F) -> SessionEvent
where
    F: Fn(&SessionEvent) -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(e)) if pred(&e) => return e,
            Ok(Ok(_)) => continue,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            other => panic!("event not received in {secs}s: {other:?}"),
        }
    }
}

#[tokio::test]
async fn real_kernel_session_index_check_run_figures_equations() {
    let Some(py) = tooling_python() else {
        eprintln!(
            "SKIP: no tooling interpreter (set ANALYST_TOOLING_PYTHON or run `uv sync` in python/kernel)"
        );
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let mut opts = WorkbenchOptions::new(home.path().to_path_buf());
    // The kernel venv has numpy and matplotlib; use it as the session interpreter too.
    opts.session_python = Some(py.to_string_lossy().to_string());
    opts.kernel_launcher = Some(vec![
        py.to_string_lossy().to_string(),
        "-m".into(),
        "analyst_kernel".into(),
    ]);
    opts.sandbox = Some("none".into());
    opts.autostart_kernel = true;
    opts.init = analyst_core::init::InitOptions {
        run_uv: false,
        git: true,
    };
    let wb = Arc::new(Workbench::new(opts).unwrap());
    let info = wb
        .create_session(&repo.path().to_string_lossy(), None, None, true)
        .await
        .unwrap();
    let sid = info.summary.id.clone();
    let rt = wb.session(&sid).unwrap();
    let mut rx = rt.subscribe();

    // Kernel starts lazily in the background.
    let ready = wait_for(&mut rx, 60, |e| {
        matches!(
            e,
            SessionEvent::KernelState {
                state: KernelState::Ready | KernelState::Error,
                ..
            }
        )
    })
    .await;
    assert!(
        matches!(
            ready,
            SessionEvent::KernelState {
                state: KernelState::Ready,
                ..
            }
        ),
        "{ready:?}"
    );
    let info = wb.session_info(&sid).await.unwrap();
    assert_eq!(info.kernel, KernelState::Ready);
    assert_eq!(info.sandbox.as_deref(), Some("none"));
    assert!(info.python_abs.is_some());

    // Index of the empty template.
    let idx = rt.index_get().await.unwrap();
    assert!(idx.scripts.is_empty());

    // Write the library and a compliant script through fs_write.
    rt.fs_write("analysis/models.py", MODELS, None).unwrap();
    rt.fs_write("scripts/decay_sim.py", SCRIPT, None).unwrap();
    // The watcher refreshes the index for .py changes (an earlier `index_changed` from the
    // kernel warm-up may still be queued, so wait until the refreshed index has the script).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let idx = loop {
        wait_for(&mut rx, 30, |e| matches!(e, SessionEvent::IndexChanged)).await;
        let idx = rt.index_get().await.unwrap();
        if !idx.scripts.is_empty() {
            break idx;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "index never refreshed: {idx:?}"
        );
    };
    assert_eq!(idx.scripts.len(), 1, "{idx:?}");
    assert_eq!(idx.scripts[0].name, "decay_sim");
    assert_eq!(idx.scripts[0].options.len(), 4);
    assert!(idx
        .library
        .iter()
        .any(|f| f.qualname == "single_exp" && f.has_math));

    // Contract + lint: no errors on the compliant script.
    let diags = rt
        .check_run(Some(vec!["scripts/decay_sim.py".into()]))
        .await
        .unwrap();
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    let d = wait_for(&mut rx, 30, |e| {
        matches!(e, SessionEvent::Diagnostics { .. })
    })
    .await;
    assert!(
        matches!(d, SessionEvent::Diagnostics { paths, .. } if paths.contains(&"scripts/decay_sim.py".to_string()))
    );

    // Run it and collect the run events.
    let view = rt
        .run_start("scripts/decay_sim.py", vec!["--n".into(), "7".into()], None)
        .await
        .unwrap();
    assert_eq!(view.meta.status, RunStatus::Running);
    assert_eq!(view.meta.args, vec!["--n", "7"]);
    let run_id = view.meta.run_id.clone();
    let started = wait_for(&mut rx, 30, |e| {
        matches!(e, SessionEvent::RunStarted { .. })
    })
    .await;
    assert!(matches!(started, SessionEvent::RunStarted { run } if run.meta.run_id == run_id));
    let mut stdout = String::new();
    let mut figure: Option<FigureView> = None;
    let mut done: Option<RunView> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while done.is_none() {
        let e = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(e)) => e,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            other => panic!("run did not finish: {other:?} (stdout so far: {stdout:?})"),
        };
        match e {
            SessionEvent::RunOutput {
                run_id: r,
                stream: OutputStream::Stdout,
                chunk,
            } if r == run_id => stdout.push_str(&chunk),
            SessionEvent::RunArtifact {
                run_id: r,
                artifact,
                figure: f,
            } if r == run_id && artifact.kind == ArtifactKind::Figure => {
                if f.is_some() && figure.is_none() {
                    figure = f;
                }
            }
            SessionEvent::RunDone { run } if run.meta.run_id == run_id => done = Some(run),
            _ => {}
        }
    }
    let done = done.unwrap();
    assert_eq!(done.meta.status, RunStatus::Done, "{done:?}");
    assert_eq!(done.meta.exit_code, Some(0), "stderr: {}", done.stderr_tail);
    assert!(stdout.contains("samples 7"), "{stdout:?}");
    assert!(stdout.contains("done"));
    assert!(done.stdout_tail.contains("samples 7"));
    let figure = figure.expect("a captured figure");
    assert!(figure.path.starts_with("out/decay_sim/"), "{}", figure.path);
    assert!(figure.name.starts_with("decay"), "{}", figure.name);
    assert_eq!(figure.run_id, run_id);
    let sidecar = figure.sidecar.as_ref().expect("sidecar");
    assert_eq!(sidecar.title.as_deref(), Some("decay"));
    assert_eq!(sidecar.axes[0].xlabel.as_deref(), Some("t"));
    assert!(repo.path().join(&figure.path).is_file());

    // figures_list groups by run, newest first, with the sidecar.
    let groups = rt.figures_list(None);
    assert!(!groups.is_empty());
    let g = groups.iter().find(|g| g.run.meta.run_id == run_id).unwrap();
    assert!(g.figures.iter().any(|f| f.path == figure.path));
    assert_eq!(g.run.meta.status, RunStatus::Done);
    let data = rt.figure_data(&figure.path).await.unwrap();
    assert!(!data.base64.is_empty());

    // runs_list / run_get.
    let runs = rt.runs_list(None);
    assert_eq!(runs[0].meta.run_id, run_id);
    let detail = rt.run_get(&run_id).unwrap();
    assert!(detail.stdout.contains("samples 7"));

    // out_list / out_read through the kernel.
    let listed = rt.out_list(None, Some(run_id.clone())).await.unwrap();
    assert!(listed.entries.iter().any(|e| e.path == figure.path));

    // Equations.
    let eqs = rt.eq_extract("analysis/models.py").await.unwrap();
    let eq = eqs.iter().find(|e| e.function == "single_exp").unwrap();
    assert!(eq.latex.is_some(), "{eq:?}");
    let check = rt
        .eq_check("analysis/models.py", "single_exp")
        .await
        .unwrap();
    assert_ne!(
        check.status,
        analyst_protocol::kernel::EqCheckStatus::Mismatch,
        "{check:?}"
    );

    // Packages (kernel env.packages).
    let pkgs = rt.env_packages().await.unwrap();
    assert!(pkgs.iter().any(|p| p.name.eq_ignore_ascii_case("numpy")));

    wb.close_session(&sid).await.unwrap();
    assert!(matches!(
        wait_for(&mut rx, 10, |e| matches!(
            e,
            SessionEvent::KernelState { .. }
        ))
        .await,
        SessionEvent::KernelState {
            state: KernelState::Stopped,
            ..
        }
    ));
}
