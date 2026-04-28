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
