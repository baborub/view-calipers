# Publishing a release

The public repository is [baborub/view-calipers](https://github.com/baborub/view-calipers). Publish from `main`; never force-push an existing release tag.

1. Set the version in Cargo.toml and Cargo.lock, update CHANGELOG.md and dependency notices, and add `docs/releases/vX.Y.Z.md`.
2. Review screenshots and staged files. Keep local media, test output, build artifacts and abandoned frontend prototypes out of source control.
3. Run the full local suite with FFmpeg and Blender installed: `cargo test --release --locked`.
4. Build release assets with `powershell -File scripts/package.ps1`. Extract the ZIP into a clean directory, verify its checksum and smoke-test the executable. The ZIP includes dependency notices; do not upload the EXE as a source file.
5. Commit and push `main`, then wait for the Windows build workflow to pass. CI installs FFmpeg and tests video; Blender import tests run locally and are excluded from CI.
6. Create an annotated version tag matching Cargo.toml and push it. For example:

   ```powershell
   git tag -a v0.5.1 -m "view-calipers v0.5.1"
   git push origin v0.5.1
   ```

The tag workflow builds and packages that exact commit and creates a **draft** release with the versioned notes, Windows x64 ZIP and SHA256SUMS.txt. Review the draft and assets, then publish it and mark it as the latest release. The workflow never publishes a draft automatically.

For an authorized manual fallback, attach only the current version's ZIP and matching SHA256SUMS.txt from `dist` and use the versioned notes file. Verify the remote tag matches the tested source commit before publishing.

Use `docs/REPOSITORY_ABOUT.md` for the repository description and topics. The `.gitignore` rules preserve local files while excluding them from publication.
