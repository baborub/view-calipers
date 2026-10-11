# Media engine performance — 0.1.1

Measured locally on 2026-10-08 with a release build, Intel Core i5-13400F (16 logical processors), NVIDIA RTX 4090, and FFmpeg 9.0.2. These are synthetic isolated benchmarks, not guarantees for every file or complete folder load times. File reading, archive extraction, disk-cache writes, GPU uploads, and UI drawing are outside the JPEG benchmark.

| Operation | Previous path | Updated path | Local result |
| --- | --- | --- | --- |
| 6000 × 4000 JPEG → 320 × 213 thumbnail | Full raster decode + resize | Reduced JPEG IDCT + resize | 79.6 → 40.6 ms, about 1.96× faster |
| 1080p/60 video → 960 × 540 poster | New FFmpeg process + PNG + image decode | New FFmpeg process + uncompressed PPM | 228.0 → 212.6 ms, about 1.07× faster |
| Ten forward scrub positions, 0.0–0.9 s | New FFmpeg process + PNG at each position | One raw RGB decoder reused across positions | 28.8 ms/seek including initial startup; about 7.9× below the measured standalone PNG extraction time |

The standalone extraction and sequential scrubbing workloads differ. The last ratio describes this neighboring-seek case, not random seeks or playback throughput. Each standalone extraction path uses five iterations; sequential scrubbing uses ten. Times include FFmpeg startup where applicable. Repeating a benchmark can change results because of process warmup, operating-system caches, and concurrent workload.

## Changes

- JPEG uses supported 1/8, 1/4, or 1/2 reduced IDCT resolution when possible, followed by the established thumbnail resize. EXIF orientation is applied before the final resize. CMYK / high-bit-depth or unsupported JPEG cases use the existing decoder.
- Thumbnail worker count is half the available CPU capacity, clamped to 1–4 per pane. Visible-first queuing, cancellation, and disk-cache invalidation remain in place.
- Video posters use uncompressed RGB PPM; the preview decodes a poster once.
- Playback uses a bounded frame queue, recycled RGB byte buffers (three rather than four bytes per pixel), and one color conversion per displayed frame. Catch-up frames are skipped before that conversion.
- Source average frame rate replaces the fixed 24 fps rate, with a 120 fps cap and 24 fps fallback for unavailable metadata. Variable-frame-rate clips are normalized to the reported average rate.
- Scrubbing keeps a decoder open for forward seeks within one second of its next frame. A large jump or uncached reverse seek starts a new decoder. Eight recent frames are retained per scrubber; previews are independent.
- Results carry request IDs and occupy one latest-result slot. The UI rejects obsolete results and updates the existing texture.
- Decoder processes are terminated when the scrubber / player is dropped. Hidden previews and closed tabs retain the existing playback pause / cleanup behavior.

## Validation and reproduction

Install FFmpeg / FFprobe / FFplay and Blender, then run:

```powershell
cargo test --release --locked -- --test-threads=1
cargo test --release --locked benchmark_ -- --ignored --nocapture --test-threads=1
```

The decoder benchmark verifies one decoder start for ten nearby forward positions and cache reuse on reverse seeks, including frames retained in the shared RAM cache. Normal regression tests cover all eight EXIF orientations, folder / archive / disk-cache previews, malformed files, 60 fps video metadata and playback, portrait / rotation metadata, A–B repeat, and independent simultaneous video previews.

## Remaining limits

Playback resolution remains approximately 960 × 540. FFmpeg hardware decoding still copies pixels to CPU memory, followed by OpenGL texture upload. This change does not implement GPU zero-copy. Audio still runs through FFplay. Random seeks still incur process startup. A future libmpv / FFmpeg API backend can address those paths without rewriting the application in another language.


## 0.1.2: 128 GiB / RTX 4090 profile

The cache stores decoded RGB preview frames. Windows already caches ordinary file reads in system RAM; copying compressed bytes into a separate application buffer would not remove video decoding work. No unbounded whole-video copy or RAM disk is required.

The shared LRU counts RGB payload bytes, keys entries by canonical file path / size / modification time / preview geometry / normalized frame rate, and is reused by both panes and reopened tabs. The cap applies to cache payloads, not the application's entire memory footprint. Foreground queues, a legacy eight-frame local cache per scrubber, textures and decoder buffers add a bounded overhead. Settings permit 0–32768 MiB. Defaults are 8192 MiB on systems with at least 64 GiB physical RAM, 2048 MiB on systems with at least 32 GiB, and 512 MiB otherwise. Only the high-memory default enables preloading automatically.

Background sampling is about 2 fps after source-rate normalization. Selection happens before resize and download. Compatible files use NVDEC / CUDA frames, `scale_cuda`, then download only the small selected previews. Rotated clips and unsupported pipelines use software. A first CUDA attempt is checked by successfully reading pixels; failed startup falls back to CPU. Two preload jobs are allowed globally, and each yields while foreground scrub decoding is active. Jobs stop when preloading is disabled, the cache epoch changes, the preview closes, or free physical RAM falls below 10% of total (minimum 2 GiB). Foreground cache growth also stops under that pressure. Failed / cancelled jobs have a retry delay to avoid repeated process startup.

Preload fills at most 75% of the global budget. It can take time to reach later parts of a long clip, and clips can exceed capacity. Drag feedback can use a nearby cached sample up to about 0.26 s away; a committed seek always requests the exact normalized frame. Precise foreground decoding populates all traversed frames, so revisiting those positions avoids decoding too.

### Measured cache-hit workload

Same i5-13400F / RTX 4090 machine, 128 GiB installed RAM, release build and FFmpeg 9.0.2. The manual test uses a **512 MiB** limit to exercise bounded behavior rather than allocating 8 GiB. Clips are synthetic 1920 × 1080, 60 fps, four seconds long, scaled to 960 × 540.

| Path | Sparse preload | RAM samples | Eight random cache-hit seeks |
| --- | --- | --- | --- |
| H.264, confirmed CUDA decode / resize | 771 ms | 8 frames / 11 MiB | 2.12 ms/seek |
| FFV1, automatic software fallback | 1002 ms | 8 frames / 11 MiB | 1.69 ms/seek |

The first cold CUDA preload was observed at about 6.8 seconds; a subsequent run produced the table above. Startup and driver / operating-system caches affect these numbers. All preloading runs in the background. Cache-hit seek timings include the worker handoff and RGB-to-color conversion, but exclude the GPU texture upload and GUI redraw. They do not describe cold or uncached random seeks.

Source-time checks compare the cached frame at 2.0 s with a directly extracted frame at 2.0 s. GPU / software interpolation may differ slightly (mean RGB byte differences 1.254 and 0.470 in this run). The manual test additionally exercises rotation metadata and cancellation. Unit tests check byte limits, foreground reserve, LRU ordering, file identity changes, and settings persistence.

Reproduce:

```powershell
cargo test --release --locked benchmark_ram_preload_cuda_and_random_scrubbing -- --ignored --nocapture --test-threads=1
```

References: [Windows Cache Manager](https://learn.microsoft.com/en-us/windows-server/administration/performance-tuning/subsystem/cache-memory-management/) and [FFmpeg CUDA scaling implementation](https://ffmpeg.org/doxygen/trunk/vf__scale__cuda_8c_source.html).


## Photo preloading — 0.2.5

Full-resolution raster photos, including archive entries, use two shared background workers. The selected file is queued first. Stale queued jobs are skipped; replies carry a per-browser generation so a completed old selection cannot replace the new one. Completed pixels can remain useful in the bounded shared cache. Closing/switching tabs cancels pending work; a pending photo resumes when that tab is activated again.

The RAM cache uses RGBA byte accounting and LRU eviction, with a 128-entry cap. Defaults are 4096 MiB on machines whose video-cache high-memory default is at least 8192 MiB, otherwise 512 MiB. The GPU texture cache is shared per egui context, defaults to 1024 MiB, and accounts estimated RGBA texture bytes. RAM settings allow 0–16384 MiB; textures allow 0–4096 MiB. Zero disables that cache while still allowing the foreground photo to load. Current previews, worker decode buffers and graphics-driver overhead are outside these cache budgets.

The RAM neighborhood contains up to three photos in the navigation direction and one behind. Up to three nearest available neighbors are candidates for GPU warming. Speculative uploads run one at a time per idle UI pass, do not evict existing textures, and pause for active pointer, wheel or key input. The UI holds texture handles; warm hits reuse the same handle rather than decoding or uploading again. Source path, archive entry, source file size and modification time identify cache entries. EXIF orientation and the existing 8192-pixel texture limit apply during background decode.

Photo decompression remains on CPU; scaling, panning and drawing use the existing OpenGL GPU renderer. This change adds preloading and caching, rather than a CUDA JPEG decoder.

Run the synthetic photo benchmark:

```powershell
cargo test --release --locked benchmark_photo_preload -- --ignored --nocapture --test-threads=1
```

The benchmark uses generated 6000×4000 JPEGs. It measures the cold UI request, background decode plus handoff, and 100 warm texture-cache requests. It does not render a native viewport or include actual GPU uploads or display-frame latency. Results depend on image content, disk, CPU and warmup.

Measured locally on the i5-13400F / RTX 4090 / 128 GiB machine: cold UI request **0.745 ms**, background decode + handoff **196.642 ms**, mean warm texture-cache request **0.092 ms** (100 requests). These synthetic measurements exclude native GPU upload and frame rendering.


## 0.2.12: native OpenGL photos and YUV video (2026-10-10)

The frontend remains egui 0.29 / eframe Glow. Opaque photo pixels now go directly from the decode worker's RGBA buffer to an OpenGL texture. A custom egui paint callback draws the photo quad. The complete mip pyramid uses immutable storage allocated before level-zero upload, followed by GPU mip generation. Reserving the pyramid first avoids the large driver reallocation observed with mutable texture storage on the RTX 4090. Transparent images retain egui's premultiplied-alpha path. Drivers without the required photo storage support use the managed fallback.

Playback, exact scrubbing and the shared RAM cache now use planar YUV420. FFmpeg normalizes matrix/range to limited-range BT.709; the fragment shader converts YUV to RGB. Each preview reuses its three plane textures at the same dimensions, including odd portrait sizes. CPU frame buffers are recycled, and scrub-cache references share their pixel allocation. The decoded cache payload is approximately half RGB24; GPU upload traffic is 1.5 instead of 4 bytes/pixel for even dimensions. Playback uses frame deadlines and decoder wakeups instead of fixed 16 ms polling. GPU deletion stays on the UI/render thread.

### Synchronized renderer measurements

Release build, i5-13400F, RTX 4090, 128 GiB RAM. Old and new paths alternate in the same real GL context. Each sample includes CPU preparation, texture upload and offscreen drawing, with `glFinish` before stopping the timer. Five warmups plus 50 measured samples per video path; three warmups plus 12 photo samples. Medians use the upper-middle sample; P95 uses sorted index floor(n * .95), capped to n - 1.

| Ready-frame workload | Old median, ms | New median, ms | Old P95, ms | New P95, ms |
| --- | ---: | ---: | ---: | ---: |
| Video 960 x 540 | 0.97 | 0.17 | 1.28 | 0.28 |
| Video 1920 x 1080 | 3.39 | 0.53 | 3.88 | 0.87 |
| Video 3840 x 2160 | 13.98 | 1.59 | 15.13 | 2.14 |
| Photo 6000 x 4000 | 56.58 | 16.69 | 59.29 | 17.31 |

The photo result includes mipmaps; a separate stage check measured mip generation at 0.16 ms. These are renderer-stage measurements, excluding file I/O, decode, UI scheduling, vsync and physical display latency. The old photo ColorImage conversion is included here although it ran on a worker in production. The 1080p/4K video inputs are synthetic renderer stress cases: actual playback resolution remains approximately 960 x 540.

### Navigation in the actual application

Two runs per version in before/after/after/before order; 12 real 24-megapixel JPEGs, 46 transitions per run, 184 total. Both versions use 4096 MiB photo RAM cache, 1024 MiB GPU cache, prefetch and the same 1600 x 1000 viewport. OS file caches were not flushed and background activity was not controlled. This is a small local sample, not a cross-machine benchmark.

| Navigation phase | Old median, ms | New median, ms | Old P95, ms | New P95, ms |
| --- | ---: | ---: | ---: | ---: |
| first-forward | 53.26 | 29.20 | 510.68 | 505.97 |
| return | 16.65 | 16.94 | 28.44 | 23.25 |
| rapid-forward | 16.68 | 16.88 | 29.55 | 41.60 |
| rapid-return | 16.72 | 16.87 | 31.47 | 25.84 |

The first forward pass improved; cached navigation remains around 17 ms. Rapid-forward P95 regressed in this sample and cold-tail latency remains about 0.5 s. Mipmaps use roughly one-third more texture memory, so the same GPU budget holds fewer photos; this is a possible contributor, not a profiled attribution. GPU warming remains bounded by the configured byte budget.

The navigation metric records texture readiness plus subsequent egui passes, not a GPU presentation fence. Native readiness includes the immediate upload, whereas the legacy egui upload is deferred. It is a UI render-opportunity proxy, not an input-to-photon measurement. Do not interpret renderer speedups as equal improvements in total application latency.

### Validation and reproduction

- Release regression suite: 69 passed, 0 failed, 6 opt-in benchmarks ignored.
- Real GPU readback: YUV colors/orientation, odd dimensions, two independent previews, reuse of three textures per stream, resource deletion, complete photo mip allocation and transparent-image fallback.
- FFmpeg color comparisons: BT.601 limited, BT.709 limited and BT.709 full sources. Existing orientation, A-B loop and concurrent-video tests pass.
- CUDA cache benchmark: H.264 preload 616 ms, 8 samples / about 5 MiB, confirmed CUDA; cached random seek 1.54 ms. FFV1 software fallback: 853 ms, 8 samples / about 5 MiB, 1.67 ms/seek. These cache-hit times exclude upload/redraw and do not describe uncached seeks.
- Native UI smoke check: two simultaneous landscape/portrait videos, timeline interaction, paired fullscreen, photo preview and photo fullscreen. Test profiles and generated color-chart media were isolated from the user's profile. No renderer errors were logged.
- A five-second settled-photo sample used 0.078 CPU seconds (1.56% of one logical CPU); this is a short CPU observation, not proof of zero idle load or a GPU utilization measurement.

```powershell
cargo test --release --locked --offline -- --test-threads=1
cargo test --release --locked --offline benchmark_ram_preload_cuda_and_random_scrubbing -- --ignored --nocapture --test-threads=1
./scripts/benchmark-media.ps1 -Exe ./target/release/view-calipers.exe -OutputDirectory ./media-benchmark
```

The diagnostic runs offline on generated pixels and exits after writing JSON and stderr. Set `VIEW_CALIPERS_MEDIA_LEGACY=1` before ordinary startup to compare the managed RGB renderer manually; remove it to restore the default native path.

Photo decoding still runs on CPU. Video frames still cross CPU memory after decoding; this release does not add Media Foundation/DXGI zero-copy, float/HDR textures, gigapixel tiling or a general photo texture-allocation pool. Existing LRU caching reuses warm photo textures. The release profile remains opt-level 3 / thin LTO; compiler changes are not responsible for these results.
