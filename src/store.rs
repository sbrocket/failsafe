use crate::util::*;
use anyhow::{format_err, Context as _, Result};
use fs2::FileExt;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    io::SeekFrom,
    marker::PhantomData,
    path::{Path, PathBuf},
};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Mutex,
};

async fn open_read_append(path: impl AsRef<Path>) -> Result<File> {
    Ok(OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
        .await?)
}

#[derive(Debug, Clone)]
pub struct PersistentStoreBuilder {
    store_dir: PathBuf,
}

impl PersistentStoreBuilder {
    /// Create a new PersistentStoreBuilder that will create PersistentStores in the given
    /// directory.
    pub async fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let store_dir = dir.into();
        if fs::create_dir(&store_dir).await.is_err() {
            if !fs::metadata(&store_dir)
                .await
                .with_context(|| format!("Failed to get dir metadata: {}", store_dir.display()))?
                .is_dir()
            {
                return Err(format_err!(
                    "File already exists, can't create directory: {}",
                    store_dir.display()
                ));
            }
        }
        Ok(PersistentStoreBuilder { store_dir })
    }

    /// Create a new PersistentStoreBuilder for the given subdirectory.
    pub async fn new_scoped(&self, dir: impl AsRef<Path>) -> Result<Self> {
        Self::new(self.store_dir.join(dir.as_ref())).await
    }

    /// Delete the directory that this PersistentStoreBuilder represents, along with all contents.
    pub async fn delete(self) -> Result<()> {
        Ok(fs::remove_dir_all(&self.store_dir).await?)
    }

    pub async fn build<T, P: AsRef<Path>>(&self, name: P) -> Result<PersistentStore<T>> {
        let path = self.store_dir.join(name.as_ref());
        let file = open_read_append(&path)
            .await
            .with_context(|| format!("Failed to open store file: {}", path.display()))?;

        let std_file = file
            .try_into_std()
            .expect("No operations should be in-flight");
        std_file.try_lock_exclusive().with_context(|| format!("Failed to lock store file ({}) exclusively; was a store with this name already created?", path.display()))?;

        Ok(PersistentStore {
            path,
            file: Mutex::new(File::from_std(std_file)),
            data_type: Default::default(),
        })
    }
}

#[derive(Debug)]
pub struct PersistentStore<T> {
    path: PathBuf,
    file: Mutex<File>,
    data_type: PhantomData<T>,
}

impl<T> PersistentStore<T>
where
    T: Default + Serialize + DeserializeOwned,
{
    pub async fn load(&self) -> Result<T> {
        let mut file = self.file.lock().await;
        file.seek(SeekFrom::Start(0))
            .await
            .context("Couldn't seek to start of file")?;

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .await
            .context("Failed to read store file")?;

        // The file might be empty if it was just created, in which case we return the default.
        if bytes.is_empty() {
            return Ok(T::default());
        }

        let value = serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "Failed to deserialize store file as {}",
                std::any::type_name::<T>()
            )
        })?;
        Ok(value)
    }

    pub async fn store(&self, value: &T) -> Result<()> {
        let json = serde_json::to_vec(value)
            .with_context(|| format!("Failed to serialize {}", std::any::type_name::<T>()))?;

        // Lock the file before doing the atomic write.
        let mut file = self.file.lock().await;

        // Atomically write to the store file through a tempfile.
        let (temppath, mut tempfile) = tempfile().await.context("Unable to create tempfile")?;
        tempfile
            .write_all(&json)
            .await
            .context("Failed to write store file")?;
        tempfile
            .flush()
            .await
            .context("Failed to flush store file")?;
        std::mem::drop(tempfile);

        fs::rename(temppath, &self.path)
            .await
            .context("Failed to atomically replace event store")?;

        // Reopen the file now that its been replaced.
        *file = open_read_append(&self.path)
            .await
            .with_context(|| format!("Failed to reopen store file: {}", self.path.display()))?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::event::Event;
    use tempdir::TempDir;

    const TEMPDIR_PREFIX: &'static str = "PersistentStore_test";

    #[tokio::test]
    async fn test_store_name_collisions() {
        let tempdir = TempDir::new(TEMPDIR_PREFIX).unwrap();
        let builder = PersistentStoreBuilder::new(tempdir.path()).await.unwrap();

        let _store = builder.build::<String, _>("foo").await.unwrap();
        assert!(builder.build::<String, _>("foo").await.is_err());
        assert!(builder.new_scoped("foo").await.is_err());

        let builder = builder.new_scoped("bar").await.unwrap();
        let _store = builder.build::<String, _>("bar").await.unwrap();
        assert!(builder.build::<String, _>("bar").await.is_err());
        assert!(builder.new_scoped("bar").await.is_err());
    }

    #[tokio::test]
    async fn test_store_load() {
        let tempdir = TempDir::new(TEMPDIR_PREFIX).unwrap();
        let builder = PersistentStoreBuilder::new(tempdir.path()).await.unwrap();
        let store = builder.build::<Event, _>("foo").await.unwrap();

        let mut event = Event::default();
        event.description = "foobar".to_owned();

        store.store(&event).await.unwrap();
        assert_eq!(store.load().await.unwrap(), event);
    }
}
