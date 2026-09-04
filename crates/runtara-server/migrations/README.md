# Server migrations

## An applied migration is immutable — comments included

sqlx stores a checksum of every migration it applies. Changing a file that has
already run makes it refuse *the entire remaining chain*, not just that file, so
every migration added afterwards silently never applies.

This has happened once. A commit tidying stale documentation links edited one
comment line in `20260716000000_workflow_slug.sql`:

```diff
--- silently reusable by a different workflow (docs/workflow-slug-plan.md,
+-- silently reusable by a different workflow (slug plan,
```

That one line changed the file's checksum. Production had already applied the
original, so from the next deploy onwards sqlx refused to migrate, and the three
migrations added after it — including the `execution_outbox` table an every-250ms
worker depends on — were never created. It went unnoticed for a week because the
server logged a warning and started anyway.

So: **never edit a migration that may have been applied anywhere.** Not the SQL,
not the comments, not the whitespace. If a comment is wrong or names a document
that no longer exists, leave it — the file is a historical record of what ran,
not documentation to maintain. Correct it in a *new* migration, or in code.

## If you have already broken a checksum

Restore the file to its original bytes:

```sh
git log --follow --diff-filter=A -- crates/runtara-server/migrations/<file>   # find the adding commit
git show <commit>:crates/runtara-server/migrations/<file> > crates/runtara-server/migrations/<file>
shasum -a 384 crates/runtara-server/migrations/<file>                          # must match _sqlx_migrations
```

Compare against what the database stored:

```sql
SELECT version, description, encode(checksum, 'hex') FROM _sqlx_migrations ORDER BY version;
```

Editing `_sqlx_migrations.checksum` to match the new file also works, but hides
the drift and has to be repeated in every environment. Restoring the file fixes
all of them at once.

## Failures are fatal

Both migration paths (`main.rs` at startup and `run_server_migrations`) now abort
rather than warn. A server that boots against a schema it does not expect answers
health checks while failing exactly the requests the missing migration was for,
which is what made the incident above invisible. `SKIP_MIGRATIONS=true` still
starts without migrating, for deployments that mean it.
