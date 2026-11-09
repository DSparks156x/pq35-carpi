//! Installing a bundle: unpack, check the OS profile, switch `current`,
//! restart, health check, roll back if it isn't healthy in time.
//!
//! The steps are driven through [`System`] so tests run them in a temp
//! root with a fake service manager and a fake clock.

use crate::bundle::{self, Policy, Unpack};
use crate::layout::Layout;
use crate::manifest::{MANIFEST, Manifest};
use crate::profile;
use carpi_proto::JobEvent;
use std::path::Path;
use std::time::Duration;

/// How long the new release has to come up healthy before it's rolled back.
pub const HEALTH_WINDOW: Duration = Duration::from_secs(180);
pub const HEALTH_POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Health {
    /// carpid's socket answers `Hello`.
    pub carpid: bool,
    /// A `livi-core` process runs.
    pub livi: bool,
    /// CAN frames arrive; `None` when not checked.
    pub frames: Option<bool>,
}

impl Health {
    pub fn ok(&self) -> bool {
        self.carpid && self.livi && self.frames != Some(false)
    }

    pub fn missing(&self) -> String {
        let mut m = Vec::new();
        if !self.carpid {
            m.push("carpid doesn't answer");
        }
        if !self.livi {
            m.push("livi-core isn't running");
        }
        if self.frames == Some(false) {
            m.push("no CAN frames");
        }
        m.join(", ")
    }
}

/// The world outside the release tree.
pub trait System {
    /// Restarts the services that run from `current`.
    fn restart(&mut self) -> Result<(), String>;
    fn health(&mut self) -> Health;
    fn sleep(&mut self, d: Duration);
    /// Monotonic time.
    fn now(&self) -> Duration;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied(String),
    RolledBack {
        failed: String,
        back_to: String,
        why: String,
    },
}

pub struct Opts {
    pub window: Duration,
    pub poll: Duration,
    /// `PROFILE_VERSION` on this device.
    pub os_profile: Option<u32>,
}

fn stage(emit: &mut dyn FnMut(JobEvent), stage: &str, message: impl Into<String>) {
    emit(JobEvent::Stage {
        stage: stage.into(),
        message: message.into(),
    });
}

/// Unpacks and verifies `file` into `releases/<version>`. Returns its manifest.
pub fn unpack(
    layout: &Layout,
    file: &Path,
    policy: &Policy,
    os_profile: Option<u32>,
    emit: &mut dyn FnMut(JobEvent),
) -> Result<Manifest, String> {
    stage(emit, "verify", format!("reading {}", file.display()));
    let staging = layout.staging();
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    }
    let reader = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut sink = Unpack {
        dir: staging.clone(),
        check: |m: &Manifest| profile::check(m.min_os_profile, os_profile),
    };
    let result = bundle::read(
        std::io::BufReader::new(reader),
        policy,
        &mut sink,
        &mut |done, total| {
            emit(JobEvent::Progress {
                stage: "unpack".into(),
                done,
                total: Some(total),
            })
        },
    );
    let v = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    let version = v.manifest.version.clone();
    let commits: Vec<String> = v
        .manifest
        .commits
        .iter()
        .map(|(repo, c)| format!("{repo} {}", c.get(..7).unwrap_or(c)))
        .collect();
    stage(
        emit,
        "verify",
        format!(
            "{version} of {} ({}): {} files ok, {:?}",
            v.manifest.date,
            commits.join(", "),
            v.manifest.files.len(),
            v.trust
        ),
    );
    let io = |e: std::io::Error| format!("installing {version}: {e}");
    std::fs::write(staging.join(MANIFEST), &v.manifest_bytes).map_err(io)?;
    let dest = layout.release(&version);
    if dest.exists() {
        // Reinstall (a dev bundle with the same name): move the old tree
        // aside first; running binaries keep their inodes.
        let aside = layout.releases().join(format!(".replaced-{version}"));
        let _ = std::fs::remove_dir_all(&aside);
        std::fs::rename(&dest, &aside).map_err(io)?;
        let _ = std::fs::remove_dir_all(&aside);
    }
    std::fs::rename(&staging, &dest).map_err(io)?;
    Ok(v.manifest)
}

/// Switches to `version`, restarts, waits for health; rolls back if needed.
pub fn switch(
    layout: &Layout,
    version: &str,
    sys: &mut dyn System,
    opts: &Opts,
    emit: &mut dyn FnMut(JobEvent),
) -> Result<Outcome, String> {
    let io = |e: std::io::Error| format!("switching to {version}: {e}");
    let before = layout.current();
    // A reconnect during the restart must not mistake the preceding apply's result for this one.
    let mut st = layout.state();
    st.last_apply = Some("applying".into());
    layout.save_state(&st).map_err(io)?;
    layout.switch_to(version).map_err(io)?;
    stage(
        emit,
        "switch",
        format!(
            "current -> {version} (was {})",
            before.as_deref().unwrap_or("nothing")
        ),
    );

    stage(emit, "restart", "restarting services");
    let why = match sys.restart() {
        Err(e) => format!("restart failed: {e}"),
        Ok(()) => {
            stage(emit, "health", "waiting for carpid and LIVI");
            match wait_healthy(sys, opts) {
                Ok(()) => {
                    let mut st = layout.state();
                    st.last_apply = Some("ok".into());
                    if st.downloaded.as_deref() == Some(version) {
                        st.downloaded = None;
                    }
                    let _ = layout.save_state(&st);
                    if let Ok(removed) = layout.prune()
                        && !removed.is_empty()
                    {
                        stage(emit, "cleanup", format!("removed {}", removed.join(", ")));
                    }
                    let _ = std::fs::remove_dir_all(layout.download_dir(version));
                    return Ok(Outcome::Applied(version.into()));
                }
                Err(why) => why,
            }
        }
    };

    // Back to what ran before, if there was something.
    let mut st = layout.state();
    let Some(back) = before.filter(|b| b != version) else {
        st.last_apply = Some("failed".into());
        let _ = layout.save_state(&st);
        return Err(format!(
            "{version} isn't healthy ({why}); nothing to roll back to"
        ));
    };
    stage(emit, "rollback", format!("{why}; back to {back}"));
    let back_to = layout.rollback().map_err(|e| format!("rollback: {e}"))?;
    st.last_apply = Some("rolled_back".into());
    let _ = layout.save_state(&st);
    if let Err(e) = sys.restart() {
        return Err(format!("rolled back to {back_to}, but restart failed: {e}"));
    }
    Ok(Outcome::RolledBack {
        failed: version.into(),
        back_to,
        why,
    })
}

pub fn wait_healthy(sys: &mut dyn System, opts: &Opts) -> Result<(), String> {
    let start = sys.now();
    loop {
        let h = sys.health();
        if h.ok() {
            return Ok(());
        }
        if sys.now().saturating_sub(start) >= opts.window {
            return Err(format!(
                "not healthy after {} s: {}",
                opts.window.as_secs(),
                h.missing()
            ));
        }
        sys.sleep(opts.poll);
    }
}

/// `apply`: unpack, then switch.
pub fn apply(
    layout: &Layout,
    file: &Path,
    policy: &Policy,
    sys: &mut dyn System,
    opts: &Opts,
    emit: &mut dyn FnMut(JobEvent),
) -> Result<Outcome, String> {
    std::fs::create_dir_all(&layout.root).map_err(|e| format!("update state: {e}"))?;
    let mut st = layout.state();
    st.last_apply = Some("applying".into());
    layout
        .save_state(&st)
        .map_err(|e| format!("update state: {e}"))?;
    let result = (|| {
        let m = unpack(layout, file, policy, opts.os_profile, emit)?;
        for fw in &m.firmware {
            stage(
                emit,
                "firmware",
                format!(
                    "{} image {:?} is in the bundle; flashing it is a separate, confirmed step",
                    fw.target, fw.version
                ),
            );
        }
        switch(layout, &m.version, sys, opts, emit)
    })();
    // Failures before switch/health must not leave an earlier successful apply in status.
    let mut st = layout.state();
    if result.is_err() && st.last_apply.as_deref() == Some("applying") {
        st.last_apply = Some("failed".into());
        layout
            .save_state(&st)
            .map_err(|e| format!("update state: {e}"))?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::tests::bundle;

    /// Healthy after `healthy_after` health checks; `None` never.
    struct Fake {
        t: Duration,
        restarts: u32,
        checks: u32,
        healthy_after: Option<u32>,
    }

    impl Fake {
        fn new(healthy_after: Option<u32>) -> Self {
            Fake {
                t: Duration::ZERO,
                restarts: 0,
                checks: 0,
                healthy_after,
            }
        }
    }

    impl System for Fake {
        fn restart(&mut self) -> Result<(), String> {
            self.restarts += 1;
            self.checks = 0;
            Ok(())
        }
        fn health(&mut self) -> Health {
            self.checks += 1;
            let ok = self.healthy_after.is_some_and(|n| self.checks > n);
            Health {
                carpid: ok,
                livi: true,
                frames: None,
            }
        }
        fn sleep(&mut self, d: Duration) {
            self.t += d;
        }
        fn now(&self) -> Duration {
            self.t
        }
    }

    const OPTS: Opts = Opts {
        window: HEALTH_WINDOW,
        poll: HEALTH_POLL,
        os_profile: Some(1),
    };

    fn policy() -> Policy<'static> {
        Policy {
            key: None,
            allow_unsigned: false,
            expect_manifest: None,
            fill_from: None,
        }
    }

    fn write_bundle(dir: &Path, version: &str, profile: u32) -> std::path::PathBuf {
        let p = dir.join(Layout::bundle_name(version));
        std::fs::write(&p, bundle(version, profile, None)).unwrap();
        p
    }

    #[test]
    fn restart_status_does_not_reuse_a_previous_success() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().join("root"));
        std::fs::create_dir_all(&l.root).unwrap();
        let mut state = l.state();
        state.last_apply = Some("ok".into());
        l.save_state(&state).unwrap();
        let bundle = write_bundle(d.path(), "1.0.0", 1);
        let mut observed = false;
        let mut emit = |event| {
            if matches!(event, JobEvent::Stage { ref stage, .. } if stage == "restart") {
                assert_eq!(l.state().last_apply.as_deref(), Some("applying"));
                observed = true;
            }
        };
        apply(
            &l,
            &bundle,
            &policy(),
            &mut Fake::new(Some(0)),
            &OPTS,
            &mut emit,
        )
        .unwrap();
        assert!(observed);
        assert_eq!(l.state().last_apply.as_deref(), Some("ok"));
    }

    #[test]
    fn an_invalid_bundle_replaces_a_previous_success_with_failure() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().join("root"));
        std::fs::create_dir_all(&l.root).unwrap();
        let mut state = l.state();
        state.last_apply = Some("ok".into());
        l.save_state(&state).unwrap();
        let bad = d.path().join("invalid.tar.zst");
        std::fs::write(&bad, b"invalid bundle").unwrap();
        assert!(
            apply(
                &l,
                &bad,
                &policy(),
                &mut Fake::new(Some(0)),
                &OPTS,
                &mut |_| {}
            )
            .is_err()
        );
        assert_eq!(l.state().last_apply.as_deref(), Some("failed"));
    }

    #[test]
    fn apply_swaps_then_rolls_back_when_unhealthy() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().join("opt/carpi"));
        let events = std::cell::RefCell::new(Vec::new());
        let mut emit = |e| events.borrow_mut().push(e);

        let b1 = write_bundle(d.path(), "1.0.0", 1);
        let mut sys = Fake::new(Some(2));
        let out = apply(&l, &b1, &policy(), &mut sys, &OPTS, &mut emit).unwrap();
        assert_eq!(out, Outcome::Applied("1.0.0".into()));
        assert_eq!(l.current().as_deref(), Some("1.0.0"));
        assert!(l.release("1.0.0").join("bin/carpid").is_file());
        assert!(l.release("1.0.0").join(MANIFEST).is_file());

        let b2 = write_bundle(d.path(), "1.1.0", 1);
        apply(
            &l,
            &b2,
            &policy(),
            &mut Fake::new(Some(0)),
            &OPTS,
            &mut emit,
        )
        .unwrap();
        assert_eq!(l.current().as_deref(), Some("1.1.0"));
        assert_eq!(l.previous().as_deref(), Some("1.0.0"));

        // Never healthy: back to 1.1.0 after the window.
        let b3 = write_bundle(d.path(), "1.2.0", 1);
        let mut sys = Fake::new(None);
        let out = apply(&l, &b3, &policy(), &mut sys, &OPTS, &mut emit).unwrap();
        assert!(matches!(
            &out,
            Outcome::RolledBack { failed, back_to, .. } if failed == "1.2.0" && back_to == "1.1.0"
        ));
        assert!(sys.t >= HEALTH_WINDOW && sys.t < HEALTH_WINDOW + HEALTH_POLL * 2);
        assert_eq!(sys.restarts, 2);
        assert_eq!(l.current().as_deref(), Some("1.1.0"));
        assert_eq!(l.state().last_apply.as_deref(), Some("rolled_back"));
        assert!(
            events
                .borrow()
                .iter()
                .any(|e| matches!(e, JobEvent::Stage { stage, .. } if stage == "rollback"))
        );

        // Two releases kept after a good apply.
        let b4 = write_bundle(d.path(), "1.3.0", 1);
        apply(
            &l,
            &b4,
            &policy(),
            &mut Fake::new(Some(0)),
            &OPTS,
            &mut emit,
        )
        .unwrap();
        let mut left: Vec<_> = std::fs::read_dir(l.releases())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["1.1.0", "1.3.0"]);
        assert_eq!(l.state().last_apply.as_deref(), Some("ok"));
    }

    #[test]
    fn first_install_that_fails_has_nothing_to_roll_back_to() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().join("root"));
        let b = write_bundle(d.path(), "1.0.0", 1);
        let err = apply(&l, &b, &policy(), &mut Fake::new(None), &OPTS, &mut |_| {}).unwrap_err();
        assert!(err.contains("nothing to roll back to"));
        assert_eq!(l.state().last_apply.as_deref(), Some("failed"));
    }

    #[test]
    fn newer_os_profile_is_refused_before_anything_changes() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().join("root"));
        let b1 = write_bundle(d.path(), "1.0.0", 1);
        apply(
            &l,
            &b1,
            &policy(),
            &mut Fake::new(Some(0)),
            &OPTS,
            &mut |_| {},
        )
        .unwrap();

        let b2 = write_bundle(d.path(), "2.0.0", 3);
        let mut sys = Fake::new(Some(0));
        let err = apply(&l, &b2, &policy(), &mut sys, &OPTS, &mut |_| {}).unwrap_err();
        assert!(err.contains("needs OS profile 3"), "{err}");
        assert_eq!(sys.restarts, 0);
        assert_eq!(l.current().as_deref(), Some("1.0.0"));
        assert!(!l.release("2.0.0").exists());
        assert!(!l.staging().exists());
    }

    #[test]
    fn a_dev_bundle_takes_the_rest_from_the_current_release() {
        use crate::bundle::tests::{FILES, tarball};
        use crate::manifest::tests::manifest_json;

        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().join("root"));
        let b1 = write_bundle(d.path(), "1.0.0", 1);
        apply(
            &l,
            &b1,
            &policy(),
            &mut Fake::new(Some(0)),
            &OPTS,
            &mut |_| {},
        )
        .unwrap();

        // Manifest lists everything; the tarball has the new carpid only.
        let mut files = FILES.to_vec();
        files[0] = ("bin/carpid", b"\x7fELF carpid dev");
        let m = manifest_json("dev-abc1234", 1, &files);
        let dev = d.path().join("dev.tar.zst");
        std::fs::write(&dev, tarball(&[(MANIFEST, &m), files[0]])).unwrap();

        // Without the base it's incomplete.
        assert!(
            apply(
                &l,
                &dev,
                &policy(),
                &mut Fake::new(Some(0)),
                &OPTS,
                &mut |_| {}
            )
            .is_err()
        );

        let base = l.release("1.0.0");
        let with_base = Policy {
            fill_from: Some(&base),
            ..policy()
        };
        apply(
            &l,
            &dev,
            &with_base,
            &mut Fake::new(Some(0)),
            &OPTS,
            &mut |_| {},
        )
        .unwrap();
        let r = l.release("dev-abc1234");
        assert_eq!(
            std::fs::read(r.join("bin/carpid")).unwrap(),
            b"\x7fELF carpid dev"
        );
        assert_eq!(
            std::fs::read(r.join("bin/livi-core")).unwrap(),
            b"\x7fELF livi"
        );

        // A base file that doesn't match the manifest fails.
        std::fs::write(r.join("bin/livi-core"), "changed").unwrap();
        let dev2 = d.path().join("dev2.tar.zst");
        let m2 = manifest_json("dev-abc1235", 1, &files);
        std::fs::write(&dev2, tarball(&[(MANIFEST, &m2), files[0]])).unwrap();
        let base = l.release("dev-abc1234");
        let with_base = Policy {
            fill_from: Some(&base),
            ..policy()
        };
        let err = apply(
            &l,
            &dev2,
            &with_base,
            &mut Fake::new(Some(0)),
            &OPTS,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.contains("bin/livi-core"), "{err}");
    }

    #[test]
    fn reinstalling_the_current_version_replaces_it() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().join("root"));
        let b0 = write_bundle(d.path(), "0.9.0", 1);
        apply(
            &l,
            &b0,
            &policy(),
            &mut Fake::new(Some(0)),
            &OPTS,
            &mut |_| {},
        )
        .unwrap();
        let b = write_bundle(d.path(), "dev-e7e3f15", 1);
        apply(
            &l,
            &b,
            &policy(),
            &mut Fake::new(Some(0)),
            &OPTS,
            &mut |_| {},
        )
        .unwrap();
        std::fs::write(l.release("dev-e7e3f15").join("bin/stale"), "x").unwrap();
        apply(
            &l,
            &b,
            &policy(),
            &mut Fake::new(Some(0)),
            &OPTS,
            &mut |_| {},
        )
        .unwrap();
        assert!(!l.release("dev-e7e3f15").join("bin/stale").exists());
        assert_eq!(l.current().as_deref(), Some("dev-e7e3f15"));
        assert_eq!(l.previous().as_deref(), Some("0.9.0"));
    }
}
