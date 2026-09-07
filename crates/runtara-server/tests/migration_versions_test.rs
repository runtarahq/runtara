use std::collections::BTreeMap;

#[test]
fn server_and_environment_migration_versions_do_not_overlap() {
    let server_migrator = sqlx::migrate!("./migrations");
    let server_versions: BTreeMap<_, _> = server_migrator
        .iter()
        .map(|migration| (migration.version, migration.description.to_string()))
        .collect();

    for migration in runtara_environment::migrations::iter() {
        if let Some(server_description) = server_versions.get(&migration.version) {
            panic!(
                "migration version {} is used by both runtara-server ({}) and runtara-environment ({})",
                migration.version, server_description, migration.description
            );
        }
    }
}

/// Two branches that number a migration the same way slip past the overlap
/// check above, because the collision sits inside one set rather than across
/// two. sqlx reports it only when a migrator first runs, as a duplicate key
/// on `_sqlx_migrations` — which for parallel merges means after main is
/// already red, in every job that touches a database.
///
/// The directories are read here rather than the embedded migrators walked,
/// because `sqlx::migrate!` records a dependency on the files it embedded and
/// so does not rebuild when a *new* file appears next to them: against a warm
/// target directory a compile-time check would keep passing on the set it was
/// last built with.
#[test]
fn no_migration_set_numbers_two_migrations_the_same() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the server crate sits under crates/");

    for relative in [
        "runtara-server/migrations",
        "runtara-store-postgres/migrations/postgresql",
        "runtara-environment/migrations",
    ] {
        let directory = crates.join(relative);
        let mut seen: BTreeMap<i64, String> = BTreeMap::new();

        for entry in std::fs::read_dir(&directory)
            .unwrap_or_else(|e| panic!("{relative} must be readable: {e}"))
        {
            let name = entry
                .unwrap_or_else(|e| panic!("{relative} entry must be readable: {e}"))
                .file_name()
                .to_string_lossy()
                .into_owned();
            if !name.ends_with(".sql") {
                continue;
            }
            let (version, _) = name
                .split_once('_')
                .unwrap_or_else(|| panic!("{relative}/{name} must start with <version>_"));
            let version: i64 = version
                .parse()
                .unwrap_or_else(|e| panic!("{relative}/{name} must start with a number: {e}"));

            if let Some(first) = seen.insert(version, name.clone()) {
                panic!("{relative} numbers two migrations {version}: {first} and {name}");
            }
        }

        assert!(!seen.is_empty(), "{relative} must hold migrations");
    }
}
