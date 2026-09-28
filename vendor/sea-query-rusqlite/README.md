This directory contains `sea-query-rusqlite` 0.8.0 from crates.io (MIT OR Apache-2.0).

The only change is its `rusqlite` dependency version, from `0.38` to `0.40.2`.
SeaORM Sync 2.0.4 otherwise links a different `libsqlite3-sys` version from
ComposeNest's bundled SQLite. Remove this patch once the upstream adapter
supports `rusqlite` 0.40.2 or later. The workspace test suite checks the
adapter against the current binding.
