use super::{Store, Tracked, types::Data};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(super) struct JsonStore {
    dir: PathBuf,
}

impl Store for JsonStore {
    fn load(&self) -> Result<Vec<Tracked>> {
        let path = self.dir.join("requests.json");
        match fs::read(&path) {
            Ok(bytes) => {
                let data: Data = serde_json::from_slice(&bytes).with_context(|| {
                    format!(
                        "Invalid tracking data in {}; repair or back up the file before retrying",
                        path.display()
                    )
                })?;
                ensure!(
                    data.version == 1,
                    "Unsupported tracking data version {}",
                    data.version
                );
                Ok(data.requests)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e).with_context(|| format!("Cannot read {}", path.display())),
        }
    }

    fn prepare(&self) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("Cannot create {}", self.dir.display()))?;
        let _probe = tempfile::NamedTempFile::new_in(&self.dir)
            .context("Tracking directory is not writable")?;
        self.load()?;
        Ok(())
    }

    /// Lock, reload, merge and atomically replace so concurrent pushes cannot lose records.
    fn save(&self, updates: Vec<Tracked>) -> Result<Vec<Tracked>> {
        self.update(|records| {
            for update in updates {
                Tracked::merge_into(records, update);
            }
        })
    }

    /// A refresh must not recreate records removed after it loaded its snapshot.
    fn save_refreshed(&self, before: &[Tracked], updates: Vec<Tracked>) -> Result<Vec<Tracked>> {
        self.update(|records| {
            for update in updates {
                let was_tracked = before.iter().any(|record| record.same_request(&update));
                let still_tracked = records.iter().any(|record| record.same_request(&update));
                if !was_tracked || still_tracked {
                    Tracked::merge_into(records, update);
                }
            }
        })
    }

    fn remove(&self, request: &Tracked) -> Result<Vec<Tracked>> {
        self.update(|records| records.retain(|record| !record.same_request(request)))
    }
}

impl JsonStore {
    pub(super) fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    fn update(&self, change: impl FnOnce(&mut Vec<Tracked>)) -> Result<Vec<Tracked>> {
        fs::create_dir_all(&self.dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.dir.join("requests.lock"))?;
        lock.lock_exclusive()?;
        let mut records = self.load()?;
        change(&mut records);
        Tracked::sort(&mut records);
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        serde_json::to_writer_pretty(
            &mut file,
            &Data {
                version: 1,
                requests: records.clone(),
            },
        )?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(self.dir.join("requests.json"))?;
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Provider, Remote, Request, Status};

    fn record(number: u64) -> Tracked {
        Tracked {
            remote: Remote {
                provider: Provider::Github,
                host: "github.com".into(),
                project: "a/b".into(),
            },
            request: Request {
                number,
                title: "test".into(),
                url: "https://github.com/a/b/pull/1".into(),
                branch: "feature".into(),
                base: "main".into(),
                status: Status::Open,
                approved: None,
                last_comment_at: None,
                unresolved_threads: None,
                updated_at: None,
                pipeline_status: None,
            },
            origins: vec![],
            checked_at: 1,
            refresh_error: None,
            pipeline_error: None,
        }
    }
    #[test]
    fn concurrent_writers_preserve_requests() {
        let dir = tempfile::tempdir().unwrap();
        let store = <dyn Store>::open(dir.path());
        let handles: Vec<_> = (1..9)
            .map(|n| {
                let store = store.clone();
                std::thread::spawn(move || store.save(vec![record(n)]).unwrap())
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(store.load().unwrap().len(), 8);
        let mut newer = record(1);
        newer.checked_at = 3;
        newer.request.status = Status::Merged;
        store.save(vec![newer]).unwrap();
        store.save(vec![record(1)]).unwrap();
        assert_eq!(store.load().unwrap()[0].request.status, Status::Merged);
    }
    #[test]
    fn removal_targets_repository_and_number_and_preserves_other_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = <dyn Store>::open(dir.path());
        let mut other_repo = record(1);
        other_repo.remote.project = "a/another-repo".into();
        store
            .save(vec![record(1), record(2), other_repo.clone()])
            .unwrap();
        // Simulate another writer adding an entry after the TUI loaded its rows.
        store.save(vec![record(3)]).unwrap();

        let remaining = store.remove(&record(1)).unwrap();
        assert_eq!(remaining.len(), 3);
        assert!(
            remaining
                .iter()
                .any(|entry| entry.same_request(&other_repo))
        );
        assert!(
            !store
                .load()
                .unwrap()
                .iter()
                .any(|entry| entry.same_request(&record(1)))
        );
        assert_eq!(store.remove(&record(1)).unwrap().len(), 3);
    }

    #[test]
    fn in_flight_refresh_does_not_restore_removed_requests() {
        let dir = tempfile::tempdir().unwrap();
        let store = <dyn Store>::open(dir.path());
        let before = store.save(vec![record(1), record(2)]).unwrap();
        let mut refreshed = before.clone();
        for entry in &mut refreshed {
            entry.request.status = Status::Merged;
            entry.checked_at = 2;
        }
        refreshed.push(record(4));

        store.save(vec![record(3)]).unwrap();
        store.remove(&record(1)).unwrap();
        let remaining = store.save_refreshed(&before, refreshed).unwrap();
        assert_eq!(
            remaining
                .iter()
                .map(|entry| entry.request.number)
                .collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        assert_eq!(remaining[0].request.status, Status::Merged);
        assert_eq!(store.load().unwrap().len(), 3);
        // An explicit push can still re-track the removed request.
        assert_eq!(store.save(vec![record(1)]).unwrap().len(), 4);
    }

    #[test]
    fn confirmed_merge_wins_over_later_refresh_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let store = <dyn Store>::open(dir.path());
        let mut stale = record(1);
        stale.request.unresolved_threads = Some(2);
        stale.checked_at = 10;
        let before = store.save(vec![stale.clone()]).unwrap();
        let mut merged = record(1);
        merged.request.status = Status::Merged;
        merged.request.unresolved_threads = Some(0);
        merged.checked_at = 9;
        // The old open snapshot can be delivered on either side of the merge.
        let saved = store.save(vec![merged]).unwrap();
        assert_eq!(saved[0].request.status, Status::Merged);
        stale.checked_at = 11;
        let saved = store.save_refreshed(&before, vec![stale]).unwrap();
        assert_eq!(saved[0].request.status, Status::Merged);
        assert_eq!(saved[0].request.unresolved_threads, Some(0));
    }

    #[test]
    fn corrupted_cache_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = <dyn Store>::open(dir.path());
        fs::write(dir.path().join("requests.json"), "broken").unwrap();
        assert!(store.save(vec![record(1)]).is_err());
        assert!(store.remove(&record(1)).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("requests.json")).unwrap(),
            "broken"
        );
    }
}
