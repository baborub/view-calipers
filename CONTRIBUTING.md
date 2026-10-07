# Contributing

Use Windows x64, stable Rust (MSVC), and Visual Studio C++ build tools.
Build with `cargo build --release --locked`.

For the complete suite, install FFmpeg, FFprobe, FFplay, and Blender, then run
`cargo test --release --locked`. Set `BLENDER_PATH` if Blender is outside the
default installation directory. CI excludes tests requiring these external tools.

Keep changes focused. For bugs, describe the steps, expected behavior, Windows
version, media format, and relevant external-tool versions. Share a minimal
non-private sample when possible. Do not commit personal media, cache files,
executables, or credentials. Test fixtures should be small and redistributable.

When dependencies change, regenerate third-party notices with
`python scripts/collect_licenses.py` and review the resulting license inventory.
Use `scripts/package.ps1` to build a release ZIP and SHA-256 checksum.
