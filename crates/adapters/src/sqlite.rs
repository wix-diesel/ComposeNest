//! Protected SQLite startup, ordered migrations, and a dedicated write worker.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

const MIGRATIONS: &[&str] = &[include_str!("../../../migrations/0001_initial.sql")];
const DATABASE_FILE: &str = "composenest.sqlite";

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// An error that prevents safe database startup or use.
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    /// A filesystem operation failed.
    #[error("database filesystem operation failed: {0}")]
    Io(#[from] io::Error),
    /// SQLite rejected an operation.
    #[error("database operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A database path is a link, has an unexpected type, or is insufficiently protected.
    #[error("unsafe database path: {0}")]
    UnsafePath(PathBuf),
    /// Another application instance currently holds the backend lock.
    #[error("another ComposeNest instance is already running")]
    AlreadyRunning,
    /// The database was written by an application with a newer schema.
    #[error("unsupported database schema version {0}")]
    NewerSchema(i64),
    /// The dedicated database worker is no longer available.
    #[error("database worker stopped")]
    WorkerStopped,
}

/// Serializes SQLite writes on one thread while holding the management-root lock.
pub struct DatabaseWorker {
    sender: Sender<Job>,
    database_path: PathBuf,
}

impl DatabaseWorker {
    /// Opens the protected database and applies pending migrations before accepting work.
    pub fn start(management_root: &Path) -> Result<Self, DatabaseError> {
        let root = management_root.to_path_buf();
        let (sender, receiver) = mpsc::channel::<Job>();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("composenest-db".into())
            .spawn(move || match open_database(&root) {
                Ok((path, mut connection, _lock)) => {
                    if ready_sender.send(Ok(path)).is_ok() {
                        for job in receiver {
                            job(&mut connection);
                        }
                    }
                }
                Err(error) => {
                    let _ = ready_sender.send(Err(error));
                }
            })?;
        let database_path = ready_receiver
            .recv()
            .map_err(|_| DatabaseError::WorkerStopped)??;
        Ok(Self {
            sender,
            database_path,
        })
    }

    /// Runs a short write operation on the dedicated database thread.
    /// External commands must finish before a transaction is started.
    pub fn write<T, F>(&self, operation: F) -> Result<T, DatabaseError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DatabaseError> + Send + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(Box::new(move |connection| {
                let _ = sender.send(operation(connection));
            }))
            .map_err(|_| DatabaseError::WorkerStopped)?;
        receiver.recv().map_err(|_| DatabaseError::WorkerStopped)?
    }

    /// Opens a separate read-only connection for a short consistent read.
    pub fn read<T, F>(&self, operation: F) -> Result<T, DatabaseError>
    where
        F: FnOnce(&Connection) -> Result<T, DatabaseError>,
    {
        let mut connection =
            Connection::open_with_flags(&self.database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let transaction = connection.transaction()?;
        let result = operation(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }
}

fn open_database(root: &Path) -> Result<(PathBuf, Connection, File), DatabaseError> {
    check_directory(root)?;
    let state = root.join("state");
    let locks = root.join("locks");
    check_directory(&state)?;
    check_directory(&locks)?;
    let lock_path = locks.join("backend.lock");
    create_or_check_file(&lock_path)?;
    let lock = OpenOptions::new().read(true).write(true).open(&lock_path)?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(DatabaseError::AlreadyRunning),
        Err(TryLockError::Error(error)) => return Err(error.into()),
    }

    let database_path = state.join(DATABASE_FILE);
    let existed = database_path.exists();
    create_or_check_file(&database_path)?;
    for suffix in ["-wal", "-shm"] {
        check_file_if_present(&state.join(format!("{DATABASE_FILE}{suffix}")))?;
    }
    let mut connection = Connection::open(&database_path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 0 || version > MIGRATIONS.len() as i64 {
        return Err(DatabaseError::NewerSchema(version));
    }
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    migrate(&mut connection, &state, existed, version)?;
    for suffix in ["-wal", "-shm"] {
        check_file_if_present(&state.join(format!("{DATABASE_FILE}{suffix}")))?;
    }
    Ok((database_path, connection, lock))
}

fn migrate(
    connection: &mut Connection,
    state: &Path,
    existed: bool,
    version: i64,
) -> Result<(), DatabaseError> {
    if version == MIGRATIONS.len() as i64 {
        return Ok(());
    }
    if existed {
        backup_before_migration(connection, state, version)?;
    }
    apply_migrations(connection, version, MIGRATIONS)
}

fn apply_migrations(
    connection: &mut Connection,
    version: i64,
    migrations: &[&str],
) -> Result<(), DatabaseError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (index, sql) in migrations.iter().enumerate().skip(version as usize) {
        transaction.execute_batch(sql)?;
        let next_version = (index + 1) as i64;
        transaction.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            [next_version],
        )?;
        transaction.pragma_update(None, "user_version", next_version)?;
    }
    transaction.commit()?;
    Ok(())
}

fn backup_before_migration(
    connection: &Connection,
    state: &Path,
    version: i64,
) -> Result<(), DatabaseError> {
    let backup_dir = state.join("migration-backups");
    if backup_dir.exists() {
        check_directory(&backup_dir)?;
    } else {
        create_private_directory(&backup_dir)?;
    }
    let mut number = 0;
    let backup_path = loop {
        let path = backup_dir.join(format!("schema-{version}-{number}.sqlite3"));
        match create_private_file(&path) {
            Ok(()) => break path,
            Err(DatabaseError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                number += 1
            }
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        connection.backup("main", &backup_path, None)?;
        let copy = Connection::open_with_flags(&backup_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let check: String = copy.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        let copied_version: i64 =
            copy.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if check != "ok" || copied_version != version {
            return Err(DatabaseError::Io(io::Error::other(
                "migration backup verification failed",
            )));
        }
        check_file_if_present(&backup_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&backup_path);
    }
    result
}

fn check_directory(path: &Path) -> Result<(), DatabaseError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || !private_permissions(&metadata, true) {
        return Err(DatabaseError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

fn check_file_if_present(path: &Path) -> Result<(), DatabaseError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && private_permissions(&metadata, false) => {
            Ok(())
        }
        Ok(_) => Err(DatabaseError::UnsafePath(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn create_or_check_file(path: &Path) -> Result<(), DatabaseError> {
    match create_private_file(path) {
        Ok(()) => Ok(()),
        Err(DatabaseError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
            check_file_if_present(path)
        }
        Err(error) => Err(error),
    }
}

fn create_private_file(path: &Path) -> Result<(), DatabaseError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), DatabaseError> {
    #[cfg(unix)]
    let mut builder = fs::DirBuilder::new();
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    check_directory(path)
}

#[cfg(unix)]
fn private_permissions(metadata: &fs::Metadata, directory: bool) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o077 == 0
        && (directory || metadata.permissions().mode() & 0o600 == 0o600)
}

#[cfg(not(unix))]
fn private_permissions(_: &fs::Metadata, _: bool) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn management_root() -> TempDir {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        create_private_directory(&root.path().join("state")).unwrap();
        create_private_directory(&root.path().join("locks")).unwrap();
        root
    }

    #[test]
    fn starts_with_required_pragmas_and_a_single_worker() {
        let root = management_root();
        let worker = DatabaseWorker::start(root.path()).unwrap();
        let settings = worker
            .write(|connection| {
                let foreign_keys: i64 =
                    connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
                let journal_mode: String =
                    connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
                let synchronous: i64 =
                    connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
                let version: i64 =
                    connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
                Ok((foreign_keys, journal_mode, synchronous, version))
            })
            .unwrap();
        assert_eq!(settings, (1, "wal".into(), 2, 1));
        assert!(matches!(
            DatabaseWorker::start(root.path()),
            Err(DatabaseError::AlreadyRunning)
        ));
        assert_eq!(
            worker
                .read(|connection| {
                    Ok(connection.query_row(
                        "SELECT count(*) FROM schema_migrations",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?)
                })
                .unwrap(),
            1
        );
    }

    #[test]
    fn rejects_newer_schema_without_migration() {
        let root = management_root();
        let path = root.path().join("state").join(DATABASE_FILE);
        create_private_file(&path).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 99).unwrap();
        drop(connection);
        assert!(matches!(
            DatabaseWorker::start(root.path()),
            Err(DatabaseError::NewerSchema(99))
        ));
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            99
        );
        assert!(!root.path().join("state/migration-backups").exists());
    }

    #[test]
    fn failed_migration_rolls_back_every_schema_change() {
        let mut connection = Connection::open_in_memory().unwrap();
        let result = apply_migrations(
            &mut connection,
            0,
            &[
                MIGRATIONS[0],
                "CREATE TABLE should_roll_back (id INTEGER); INVALID SQL;",
            ],
        );
        assert!(result.is_err());
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let count: i64 = connection.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('schema_migrations', 'should_roll_back')", [], |row| row.get(0)).unwrap();
        assert_eq!((version, count), (0, 0));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_migration_when_backup_directory_is_unprotected() {
        use std::os::unix::fs::PermissionsExt;
        let root = management_root();
        let path = root.path().join("state").join(DATABASE_FILE);
        create_private_file(&path).unwrap();
        let backup_dir = root.path().join("state/migration-backups");
        fs::create_dir(&backup_dir).unwrap();
        fs::set_permissions(&backup_dir, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(matches!(
            DatabaseWorker::start(root.path()),
            Err(DatabaseError::UnsafePath(_))
        ));
        let connection = Connection::open(&path).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 0);
    }

    #[test]
    fn backup_includes_committed_wal_data_before_migration() {
        let root = management_root();
        let path = root.path().join("state").join(DATABASE_FILE);
        create_private_file(&path).unwrap();
        let source = Connection::open(&path).unwrap();
        source.pragma_update(None, "journal_mode", "WAL").unwrap();
        source
            .execute_batch(
                "CREATE TABLE old_data (secret TEXT); INSERT INTO old_data VALUES ('from-wal');",
            )
            .unwrap();
        let worker = DatabaseWorker::start(root.path()).unwrap();
        let backup = root
            .path()
            .join("state/migration-backups/schema-0-0.sqlite3");
        let copy = Connection::open_with_flags(&backup, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let value: String = copy
            .query_row("SELECT secret FROM old_data", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "from-wal");
        assert_eq!(
            worker
                .read(|connection| {
                    Ok(
                        connection.query_row("SELECT secret FROM old_data", [], |row| {
                            row.get::<_, String>(0)
                        })?,
                    )
                })
                .unwrap(),
            value
        );
        #[cfg(unix)]
        for file in [
            &path,
            &backup,
            &root
                .path()
                .join("state")
                .join(format!("{DATABASE_FILE}-wal")),
            &root
                .path()
                .join("state")
                .join(format!("{DATABASE_FILE}-shm")),
        ] {
            let metadata = fs::metadata(file).unwrap();
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                metadata.permissions().mode() & 0o077,
                0,
                "{}",
                file.display()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_unprotected_database_and_symlinked_wal() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = management_root();
        let path = root.path().join("state").join(DATABASE_FILE);
        File::create(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            DatabaseWorker::start(root.path()),
            Err(DatabaseError::UnsafePath(_))
        ));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(
            &path,
            root.path()
                .join("state")
                .join(format!("{DATABASE_FILE}-wal")),
        )
        .unwrap();
        assert!(matches!(
            DatabaseWorker::start(root.path()),
            Err(DatabaseError::UnsafePath(_))
        ));
    }
}
