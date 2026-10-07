# Publishing v0.1.0

The project is prepared locally; no repository or release has been published yet.

1. Review README, LICENSE, THIRD_PARTY.md, the neutral screenshots, and the release notes.
2. Run the full local suite with FFmpeg and Blender installed:
   `cargo test --release --locked`.
3. Generate release assets: `powershell -File scripts/package.ps1`.
4. Initialize a repository in the project directory and inspect the staged files:

   ```powershell
   git init -b main
   git add .
   git diff --cached --stat
   git commit -m "Prepare view-calipers v0.1.0"
   ```

5. Create an empty GitHub repository under `baborub/view-calipers`, without generating additional README or license files, then push:

   ```powershell
   git remote add origin https://github.com/baborub/view-calipers.git
   git push -u origin main
   ```

6. Wait for the Windows CI build to pass. Create and push the version tag:

   ```powershell
   git tag -a v0.1.0 -m "view-calipers v0.1.0"
   git push origin v0.1.0
   ```

The tag workflow builds and packages the app, then creates a **draft** GitHub release with notes and assets. Open that draft, review it, and publish when ready. The workflow never automatically publishes the draft.

For a manual release instead, attach the ZIP and SHA256SUMS.txt from `dist`, and paste `docs/releases/v0.1.0.md` into the release description. Do not upload the application EXE as a source file; the ZIP includes license notices.

Use `docs/REPOSITORY_ABOUT.md` for the repository description and topics. The ignore rules exclude the existing local `files/`, `files.zip`, root EXEs, screenshots, and build outputs; they do not delete those files.

Before the next release, update Cargo.toml / Cargo.lock, CHANGELOG.md, license notices, and add `docs/releases/vX.Y.Z.md`. Tag and package versions must match.
