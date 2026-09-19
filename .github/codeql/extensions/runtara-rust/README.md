# Artifact-pinning false positives

GitHub's default CodeQL setup automatically loads model packs under
`.github/codeql/extensions`. This pack models only the return values of
`WorkflowExecutor::linker_with_trusted_pins` and `pin_trusted_dependencies`.
They operate on artifact hashes, metadata, imports, and linkers; they do not
resolve or return connection credentials.

CodeQL 2.27.0 classifies function calls whose names contain `trusted` as
potential sources of secret data. These two calls consequently produced 72
`rust/cleartext-logging` findings at test diagnostic assertions in PR #256.
The models stop that inferred flow at the exact call results. They do not
exclude files, test directories, queries, other `trusted` functions, arguments,
or credential-resolution paths. Function names and the public `trusted` flag
remain unchanged.

The Rust logging queries share the `log-injection` barrier kind, so these two
return-value models apply to both cleartext logging and log injection. Review
these models if either helper starts returning sensitive or arbitrary log text.
This is a repository model, not a dismissal of individual alerts; a subsequent
CodeQL analysis must verify that the original findings disappear.

References:

- [Default setup model-pack discovery](https://docs.github.com/en/code-security/how-tos/find-and-fix-code-vulnerabilities/manage-your-configuration/edit-default-setup#extending-coverage-for-a-repository)
- [Rust barrier models and canonical paths](https://codeql.github.com/docs/codeql-language-guides/customizing-library-models-for-rust/)
- [Rust cleartext logging barrier kind](https://github.com/github/codeql/blob/codeql-cli/v2.27.0/rust/ql/lib/codeql/rust/security/CleartextLoggingExtensions.qll#L44-L48)
