# Repository workflow

The sole maintainer and repository collaborator is [@abiiemmm](https://github.com/abiiemmm). `CODEOWNERS` assigns every path to that account; GitHub repository settings control actual access. Do not invite additional collaborators or add `Co-authored-by` trailers. Dependency license notices retain their original attribution.

## Development

Use the prerequisites and Windows setup in [README.md](README.md). On a fresh checkout:

```powershell
npm ci
npm run sensors:build
npm run check
npm run build
```

The sensor build prepares resources required by the Rust/Tauri build. `npm run tauri:dev` and `npm run tauri:build` also prepare those resources. Browser-only development uses `npm run dev` and does not access Windows APIs.

Create a focused branch such as `feat/game-registration`, `fix/history-retention`, or `chore/ci`. Keep implementation, relevant verification, and documentation in the same change. Run the relevant checks before pushing. Changes may use a pull request for review by the maintainer without adding another collaborator. Record concrete behavior and validation in the pull request; avoid committing local machine evidence.

Use the maintainer's GitHub identity for commits. Preserve published history and avoid force pushes to `main`. Commit messages describe the change and contain no co-author trailers. The initial license commit and its attribution remain intact.

## Dependency and source boundaries

Follow [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Commit `package-lock.json`, `src-tauri/Cargo.lock`, `sidecar/packages.lock.json`, and `global.json`. Update manifests and lockfiles together. CI uses locked restores; do not bypass them to hide a dependency mismatch. GitHub Actions are pinned to commit SHAs; review upstream changes before updating those pins.

Keep installer binaries, local SDKs, generated schemas, dependency downloads, databases, screenshots, logs, and QA reports out of Git. They are ignored under `artifacts/`, `.tools/`, and each component's build directories. Never commit credentials or signing keys. Checked-in icons and third-party license notices are source assets.

## Verification and distribution

Windows CI checks frontend contracts, translations, analytics, SQLite, Rust tests, the sensor self-test, and the unsigned NSIS build. Each successful run uploads an installer and SHA-256 manifest as a GitHub Actions artifact with 14-day retention. This is an internal build; uploading it does not certify production readiness or publish a GitHub release.

Use [PRODUCTION.md](PRODUCTION.md) to track native verification and remaining release requirements. Local reports referenced there are workspace evidence, not files shipped in this source repository. Signing, physical hardware coverage, and cross-version upgrade checks remain separate release work.
