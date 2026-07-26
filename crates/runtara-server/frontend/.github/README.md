# These workflows do not run

GitHub Actions only discovers workflows in `.github/workflows/` at the
**repository root**. This directory is `crates/runtara-server/frontend/.github/`,
so nothing here has ever executed — it is left over from when the frontend was
its own repository.

## Already handled

- `pr-checks.yml` — **ported** to `/.github/workflows/frontend.yml` and deleted
  from here. That workflow runs lint, format, typecheck, vitest, knip, build and
  the mocked Playwright project, with `working-directory` set to this directory.

## Still orphaned — decide before deleting

These have no equivalent at the repo root. They are kept because deleting them
would destroy the only copy of the configuration, not because they work.

| file                     | what it was for                                                                                                                                                              |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `deploy-bunny.yml`       | Build + deploy the frontend to Bunny CDN (260 lines). If the frontend is still deployed this way, this needs porting to the root; if not, it should be deleted deliberately. |
| `claude.yml`             | Claude Code GitHub app integration.                                                                                                                                          |
| `claude-code-review.yml` | Claude Code automated PR review.                                                                                                                                             |

To make any of them live, copy it to `/.github/workflows/`, add
`working-directory: crates/runtara-server/frontend` (or a `defaults.run` block)
to every step that runs npm, and set
`cache-dependency-path: crates/runtara-server/frontend/package-lock.json` on
`actions/setup-node`.

See `docs/crates-structure.md` (finding D3) for how this was found.
