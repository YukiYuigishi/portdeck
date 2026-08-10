//! Portable operating-system identity helpers.

/// Returns the effective user ID that owns resources created by this process.
pub(crate) fn effective_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::effective_uid;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "portdeck-platform-test-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn effective_uid_matches_the_owner_of_new_resources() {
        let directory = TestDirectory::new();

        assert_eq!(effective_uid(), fs::metadata(&directory.0).unwrap().uid());
    }
}
