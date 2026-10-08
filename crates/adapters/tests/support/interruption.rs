use std::{path::Path, process::Command};

use composenest_adapters::sqlite::DatabaseWorker;
use sea_orm::ConnectionTrait;

pub(crate) fn checkpoint(boundary: &str) {
    if std::env::var("COMPOSENEST_CRASH_BOUNDARY").as_deref() == Ok(boundary) {
        // Exit without destructors to exercise SQLite recovery and root-lock release.
        std::process::exit(86);
    }
}

pub(crate) fn child(test: &str, root: &Path, boundary: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("COMPOSENEST_CRASH_ROOT", root)
        .env("COMPOSENEST_CRASH_BOUNDARY", boundary)
        .output()
        .unwrap();
    let crashed = output.status.code() == Some(86);
    #[cfg(unix)]
    let crashed = {
        use std::os::unix::process::ExitStatusExt;
        crashed || output.status.signal() == Some(9)
    };
    assert!(crashed, "{test}/{boundary}: {output:?}");
}

pub(crate) fn full_at_completion(db: &DatabaseWorker) {
    let pages: i64 = db
        .read(|connection| Ok(connection.pragma_query_value(None, "page_count", |r| r.get(0))?))
        .unwrap();
    db.orm_write(move |connection| {
        connection.execute_unprepared(
            "CREATE TABLE capacity_probe (data BLOB);
             CREATE TRIGGER exhaust_completion BEFORE UPDATE OF status ON operations
             WHEN NEW.status='Succeeded'
             BEGIN INSERT INTO capacity_probe VALUES (zeroblob(1048576)); END;",
        )?;
        connection.execute_unprepared(&format!("PRAGMA max_page_count={}", pages + 16))?;
        Ok(())
    })
    .unwrap();
    db.write(move |connection| {
        connection.pragma_update(None, "max_page_count", pages + 16)?;
        let error = connection
            .execute("INSERT INTO capacity_probe VALUES (zeroblob(1048576))", [])
            .unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DiskFull)
        );
        Ok(())
    })
    .unwrap();
}

pub(crate) fn restore_capacity(db: &DatabaseWorker) {
    db.write(|connection| {
        connection.pragma_update(None, "max_page_count", 1073741823)?;
        Ok(())
    })
    .unwrap();
    db.orm_write(|connection| {
        connection.execute_unprepared("PRAGMA max_page_count=1073741823")?;
        connection.execute_unprepared("DROP TRIGGER exhaust_completion")?;
        Ok(())
    })
    .unwrap();
}
