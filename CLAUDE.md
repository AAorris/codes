See AGENTS.md for the working process.

## Done-check

Tasks that change artifacts — anything under `functions/`, `dev/fragments/`, or
`dev/templates/` — are done only when you have updated staging, verified it yourself,
verified it with the user, and pushed to production:

1. **Update staging.** `cargo test` passes, then upload the archive:
   `hotarc put --ref` on a branch, or `hotarc put` on `main` (preview). Exit 2 means
   someone else wrote that channel: run the `hotarc get` command it prints, reconcile,
   and put again. Exit 3 means the secret scan found something: remove it (use
   `--allow-secrets` only with the user's OK).
2. **Verify yourself.** For every changed page, request the preview through
   `vercel curl <path> --deployment <preview-url> -- -H 'x-archive-ref: <branch>' --dump-header -`.
   `x-archive-source` must be the channel you wrote (`refs/<branch>` or `preview`), and
   `x-archive-etag` must equal `.archive/<channel>.etag`. `fallback` means not done.
   Then check the page renders as intended.
   Locally, `cargo run -- --preview` serves the same S3 archive. To find the files
   behind a URL, read its `x-fragments` header or the `<!-- fragment … -->` comments.
3. **Verify with the user.** Show them what changed and where to see it, and wait for
   their OK in this conversation. An earlier OK does not carry over.
4. **Push to production.** `hotarc promote` (add `--ref <branch>` to promote
   a ref). It ships the exact bytes verified on staging; S3 checks their SHA-256.
   To undo, `hotarc rollback`, with the user's OK.

Never ship content with `git push` or `vercel --prod`. A container deploy is needed only
when `kernel/`, `build.rs`, or `Dockerfile.vercel` change, or the embedded fallback
should be refreshed.

Bucket settings live in `.archive/config.toml` (gitignored); never commit the bucket
name or account ID.
